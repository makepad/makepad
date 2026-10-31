//! `name`: renaming a family whose licence reserves its name (an OFL
//! Reserved Font Name may not be used by a modified version, and a subset is
//! one), and reading the reserved names from the licence text.

use {
    crate::{
        sfnt::{put_u16, u16_at},
        Rename, SubsetError,
    },
    std::collections::HashMap,
};

/// Name ids that tell the licence, copyright and provenance: they keep the
/// reserved name as written.
const UNTOUCHED: [u16; 10] = [0, 7, 8, 9, 10, 11, 12, 13, 14, 19];

fn rename_text(text: &str, rename: &Rename) -> String {
    let mut reserved: Vec<&String> = rename.reserved.iter().filter(|r| !r.is_empty()).collect();
    reserved.sort_by_key(|r| std::cmp::Reverse(r.len()));
    let family_compact: String = rename.family.split_whitespace().collect();
    let mut text = text.to_string();
    for name in reserved {
        text = text.replace(name.as_str(), &rename.family);
        // PostScript names drop the spaces ("PlayfairDisplay-Bold").
        let compact: String = name.split_whitespace().collect();
        if compact != *name {
            text = text.replace(&compact, &family_compact);
        }
    }
    text
}

/// The name table with every naming record (family, full, PostScript,
/// typographic and variation names, the fvar instance names) renamed.
pub(crate) fn rename(name: &[u8], rename: &Rename) -> Result<Vec<u8>, SubsetError> {
    let format = u16_at(name, 0)?;
    let count = u16_at(name, 2)? as usize;
    let storage = u16_at(name, 4)? as usize;
    let lang_tags = if format == 1 { u16_at(name, 6 + 12 * count)? as usize } else { 0 };
    let header_len = 6 + 12 * count + if format == 1 { 2 + 4 * lang_tags } else { 0 };
    let mut header = name
        .get(..header_len)
        .ok_or(SubsetError::Malformed("name records truncated"))?
        .to_vec();
    let string = |offset: usize, length: usize| {
        name.get(storage + offset..storage + offset + length)
            .ok_or(SubsetError::Malformed("name string outside the table"))
    };
    let mut strings = Vec::new();
    let mut written: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut store = |bytes: Vec<u8>, strings: &mut Vec<u8>| -> usize {
        *written.entry(bytes).or_insert_with_key(|bytes| {
            strings.extend_from_slice(bytes);
            strings.len() - bytes.len()
        })
    };
    for i in 0..count {
        let record = 6 + 12 * i;
        let platform = u16_at(name, record)?;
        let encoding = u16_at(name, record + 2)?;
        let name_id = u16_at(name, record + 6)?;
        let length = u16_at(name, record + 8)? as usize;
        let offset = u16_at(name, record + 10)? as usize;
        let bytes = string(offset, length)?;
        let renamed = if UNTOUCHED.contains(&name_id) {
            None
        } else if platform == 0 || platform == 3 {
            let units: Vec<u16> = bytes.chunks_exact(2).map(|b| u16::from_be_bytes([b[0], b[1]])).collect();
            String::from_utf16(&units)
                .ok()
                .map(|text| rename_text(&text, rename).encode_utf16().flat_map(|u| u.to_be_bytes()).collect())
        } else if platform == 1 && encoding == 0 && bytes.is_ascii() {
            let text = std::str::from_utf8(bytes).unwrap_or_default();
            let renamed = rename_text(text, rename);
            renamed.is_ascii().then(|| renamed.into_bytes())
        } else {
            None
        };
        let bytes = renamed.unwrap_or_else(|| bytes.to_vec());
        put_u16(&mut header, record + 8, bytes.len() as u16);
        let at = store(bytes, &mut strings);
        put_u16(&mut header, record + 10, at as u16);
    }
    for i in 0..lang_tags {
        let record = 6 + 12 * count + 2 + 4 * i;
        let length = u16_at(name, record)? as usize;
        let offset = u16_at(name, record + 2)? as usize;
        let at = store(string(offset, length)?.to_vec(), &mut strings);
        put_u16(&mut header, record + 2, at as u16);
    }
    if strings.len() > 0xFFFF {
        return Err(SubsetError::Malformed("renamed name strings exceed the table's offsets"));
    }
    put_u16(&mut header, 4, header_len as u16);
    header.extend_from_slice(&strings);
    Ok(header)
}

/// A format 0 name table of the records `keep` accepts (by platform,
/// language and name id); records naming a language tag are left out.
pub(crate) fn select(name: &[u8], keep: impl Fn(u16, u16, u16) -> bool) -> Result<Vec<u8>, SubsetError> {
    let count = u16_at(name, 2)? as usize;
    let storage = u16_at(name, 4)? as usize;
    let mut records = Vec::new();
    for i in 0..count {
        let record = 6 + 12 * i;
        let (platform, language, id) = (u16_at(name, record)?, u16_at(name, record + 4)?, u16_at(name, record + 6)?);
        if language < 0x8000 && keep(platform, language, id) {
            let (length, offset) = (u16_at(name, record + 8)? as usize, u16_at(name, record + 10)? as usize);
            let text = name
                .get(storage + offset..storage + offset + length)
                .ok_or(SubsetError::Malformed("name string outside the table"))?;
            records.push((&name[record..record + 8], text));
        }
    }
    let mut out = Vec::new();
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&(records.len() as u16).to_be_bytes());
    out.extend_from_slice(&((6 + 12 * records.len()) as u16).to_be_bytes());
    let mut offset = 0usize;
    for (header, text) in &records {
        out.extend_from_slice(header);
        out.extend_from_slice(&(text.len() as u16).to_be_bytes());
        out.extend_from_slice(&(offset as u16).to_be_bytes());
        offset += text.len();
    }
    for (_, text) in records {
        out.extend_from_slice(text);
    }
    Ok(out)
}

/// The Reserved Font Names an OFL notice declares ("Copyright 2017 IBM Corp.
/// with Reserved Font Name "Plex""), in the order written. The licence body's
/// own uses of the phrase (its definitions) are not declarations: a
/// declaration follows "with".
pub fn reserved_font_names(ofl_text: &str) -> Vec<String> {
    const PHRASE: &str = "reserved font name";
    let lower = ofl_text.to_ascii_lowercase();
    let mut names = Vec::new();
    let mut from = 0;
    while let Some(found) = lower[from..].find(PHRASE) {
        let at = from + found;
        from = at + PHRASE.len();
        if !lower[..at].trim_end().ends_with("with") {
            continue;
        }
        let mut rest = &ofl_text[from..];
        rest = rest.strip_prefix('s').unwrap_or(rest);
        rest = rest.trim_start_matches([' ', '\t', ':']);
        let mut quoted = false;
        loop {
            let Some(open) = rest.chars().next() else { break };
            let close: &[char] = match open {
                '"' => &['"', '”'],
                '“' => &['”', '"'],
                '\'' => &['\'', '’'],
                '‘' => &['’', '\''],
                _ => break,
            };
            let body = &rest[open.len_utf8()..];
            let Some(end) = body.find(close) else { break };
            let name = body[..end].trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
            quoted = true;
            let after = &body[end..];
            rest = after[after.chars().next().map_or(0, |c| c.len_utf8())..].trim_start_matches([' ', '\t']);
            // More names follow after ",", "and" or ", and".
            let next = rest.trim_start_matches(',').trim_start();
            match next.strip_prefix("and ") {
                Some(more) => rest = more.trim_start(),
                None if rest.starts_with(',') => rest = next,
                None => break,
            }
        }
        if !quoted {
            let end = rest.find(['.', ',', ';', '\n', '\r']).unwrap_or(rest.len());
            let name = rest[..end].trim();
            if !name.is_empty() {
                names.push(name.to_string());
            }
        }
    }
    names
}
