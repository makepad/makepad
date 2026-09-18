//! Bounded MIME decoding shared by file and future network sources.
use crate::source::MailMessage;
use makepad_html::{parse_html, HtmlNode};
use makepad_widgets::{id, InternLiveId, LiveId};

pub fn parse(raw: &[u8]) -> Result<MailMessage, String> {
    let (headers, _) = split(raw)?;
    let mut message = MailMessage {
        subject: decode_words(&header(&headers, "subject")),
        from: decode_words(&header(&headers, "from")),
        to: decode_words(&format!(
            "{} {}",
            header(&headers, "to"),
            header(&headers, "cc")
        ))
        .trim()
        .into(),
        message_id: header(&headers, "message-id"),
        date: parse_date(&header(&headers, "date")),
        ..Default::default()
    };
    let mut parts = 0;
    let mut html = String::new();
    message.body = body(
        raw,
        0,
        &mut parts,
        &mut message.attachments,
        &mut message.incomplete,
        &mut html,
    )?;
    message.presentation = if html.is_empty() {
        crate::presentation::plain(&message.body)
    } else {
        crate::presentation::sanitize(&html)
    };
    if message.subject.is_empty() {
        message.subject = "(No subject)".into();
    }
    if message.body.is_empty() {
        message.incomplete = true;
    }
    Ok(message)
}
fn split(raw: &[u8]) -> Result<(Vec<(String, String)>, &[u8]), String> {
    let at = raw
        .windows(4)
        .position(|v| v == b"\r\n\r\n")
        .map(|p| (p, 4))
        .or_else(|| raw.windows(2).position(|v| v == b"\n\n").map(|p| (p, 2)))
        .ok_or("Message headers are incomplete")?;
    if at.0 > 256 * 1024 {
        return Err("Message headers exceed decoding limit".into());
    }
    let mut headers: Vec<(String, String)> = Vec::new();
    for line in String::from_utf8_lossy(&raw[..at.0]).lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, value)) = headers.last_mut() {
                value.push(' ');
                value.push_str(line.trim());
            }
        } else if let Some((key, value)) = line.split_once(':') {
            headers.push((key.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    Ok((headers, &raw[at.0 + at.1..]))
}
fn header(headers: &[(String, String)], key: &str) -> String {
    headers
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}
fn parameter(value: &str, name: &str) -> String {
    // Quoted strings may contain semicolons and escaped quotes.
    let mut quoted = false;
    let mut escaped = false;
    let mut pieces = Vec::new();
    let mut start = 0;
    for (i, c) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if c == '\\' && quoted {
            escaped = true;
            continue;
        }
        if c == '"' {
            quoted = !quoted;
        }
        if c == ';' && !quoted {
            pieces.push(&value[start..i]);
            start = i + 1;
        }
    }
    pieces.push(&value[start..]);
    let mut extended = None;
    let mut normal = String::new();
    for piece in pieces {
        let Some((key, val)) = piece.trim().split_once('=') else {
            continue;
        };
        let val = val.trim().trim_matches('"').replace("\\\"", "\"");
        if key.trim().eq_ignore_ascii_case(name) {
            normal = decode_words(&val);
        }
        if key.trim().eq_ignore_ascii_case(&format!("{name}*")) {
            let mut chunks = val.splitn(3, '\'');
            let charset = chunks.next().unwrap_or("utf-8");
            let _language = chunks.next();
            if let Some(encoded) = chunks.next() {
                extended = Some(decode_charset(&percent(encoded), charset));
            }
        }
    }
    extended.unwrap_or(normal)
}
fn percent(text: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    let input = text.as_bytes();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' && i + 2 < input.len() {
            if let (Some(a), Some(b)) = (hex(input[i + 1]), hex(input[i + 2])) {
                bytes.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        bytes.push(input[i]);
        i += 1;
    }
    bytes
}
fn body(
    raw: &[u8],
    depth: usize,
    parts: &mut usize,
    attachments: &mut Vec<String>,
    incomplete: &mut bool,
    html: &mut String,
) -> Result<String, String> {
    *parts += 1;
    if depth > 16 || *parts > 4096 {
        return Err("MIME nesting exceeds decoding limit".into());
    }
    let (headers, bytes) = split(raw)?;
    let content = header(&headers, "content-type");
    let kind = content
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let disposition = header(&headers, "content-disposition");
    let name = parameter(&disposition, "filename");
    let name = if name.is_empty() {
        parameter(&content, "name")
    } else {
        name
    };
    if !name.is_empty() || disposition.to_ascii_lowercase().starts_with("attachment") {
        attachments.push(if name.is_empty() {
            "Unnamed attachment".into()
        } else {
            name
        });
        return Ok(String::new());
    }
    if kind.starts_with("multipart/") {
        let boundary = parameter(&content, "boundary");
        if boundary.is_empty() {
            *incomplete = true;
            return Ok(String::new());
        }
        let (subparts, closed) = subparts(bytes, &boundary);
        let mut choices: Vec<(bool, String)> = Vec::new();
        for part in subparts {
            let plain = split(part)
                .ok()
                .map(|(h, _)| {
                    header(&h, "content-type")
                        .to_ascii_lowercase()
                        .starts_with("text/plain")
                })
                .unwrap_or(false);
            match body(part, depth + 1, parts, attachments, incomplete, html) {
                Ok(text) => choices.push((plain, text)),
                Err(_) => *incomplete = true,
            }
        }
        if !closed {
            *incomplete = true;
        }
        if kind == "multipart/alternative" {
            return Ok(choices
                .iter()
                .find(|(plain, text)| *plain && !text.trim().is_empty())
                .or_else(|| {
                    choices
                        .iter()
                        .rev()
                        .find(|(_, text)| !text.trim().is_empty())
                })
                .map(|(_, text)| text.clone())
                .unwrap_or_default());
        }
        return Ok(choices
            .into_iter()
            .map(|(_, text)| text)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n"));
    }
    let bytes = transfer_decode(&headers, bytes)?;
    if kind == "message/rfc822" {
        return body(&bytes, depth + 1, parts, attachments, incomplete, html);
    }
    if !kind.is_empty() && !kind.starts_with("text/") {
        return Ok(String::new());
    }
    let text = decode_charset(&bytes, &parameter(&content, "charset"));
    let text = if kind == "text/html" {
        html.push_str(&text);
        html_text(&text)
    } else {
        text
    };
    // NUL and other controls do not belong in the index or text renderer.
    Ok(text
        .chars()
        .filter(|c| !c.is_control() || matches!(*c, '\n' | '\t'))
        .collect())
}
/// The bodies between a multipart's boundary lines, and whether the closing
/// boundary was seen.
fn subparts<'a>(bytes: &'a [u8], boundary: &str) -> (Vec<&'a [u8]>, bool) {
    let marker = format!("--{boundary}");
    let closing = format!("--{boundary}--");
    let mut start = None;
    let mut offset = 0;
    let mut closed = false;
    let mut parts = Vec::new();
    for line in bytes.split_inclusive(|b| *b == b'\n') {
        let trimmed = line.strip_suffix(b"\n").unwrap_or(line);
        let trimmed = trimmed.strip_suffix(b"\r").unwrap_or(trimmed);
        let trimmed = trimmed.trim_ascii_end();
        if trimmed == marker.as_bytes() || trimmed == closing.as_bytes() {
            if let Some(from) = start.take() {
                parts.push(&bytes[from..offset]);
            }
            if trimmed == closing.as_bytes() {
                closed = true;
                break;
            }
            start = Some(offset + line.len());
        }
        offset += line.len();
    }
    (parts, closed)
}
fn transfer_decode(headers: &[(String, String)], bytes: &[u8]) -> Result<Vec<u8>, String> {
    let encoding = header(headers, "content-transfer-encoding").to_ascii_lowercase();
    Ok(match encoding.trim() {
        "base64" => makepad_base64::base64_decode(
            &bytes
                .iter()
                .copied()
                .filter(|b| !b.is_ascii_whitespace())
                .collect::<Vec<_>>(),
        )
        .map_err(|_| "Invalid MIME base64")?,
        "quoted-printable" => quoted_printable(bytes, false),
        _ => bytes.to_vec(),
    })
}

/// One decoded attachment part.
pub struct AttachmentPart {
    pub name: String,
    pub bytes: Vec<u8>,
    /// IMAP-style part number ("2", "1.3"), which is also how Apple Mail
    /// files an attachment it stripped out of a partial message.
    pub part: String,
}

/// The `index`th attachment of the message, counted in the same order
/// `parse` lists attachment names. `Ok(None)` when there is no such part.
pub fn attachment(raw: &[u8], index: usize) -> Result<Option<AttachmentPart>, String> {
    let mut counter = 0;
    let mut parts = 0;
    find_attachment(raw, 0, &mut parts, &mut counter, index, "")
}
fn find_attachment(
    raw: &[u8],
    depth: usize,
    parts: &mut usize,
    counter: &mut usize,
    index: usize,
    part: &str,
) -> Result<Option<AttachmentPart>, String> {
    *parts += 1;
    if depth > 16 || *parts > 4096 {
        return Err("MIME nesting exceeds decoding limit".into());
    }
    let (headers, bytes) = split(raw)?;
    let content = header(&headers, "content-type");
    let kind = content
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    let disposition = header(&headers, "content-disposition");
    let name = parameter(&disposition, "filename");
    let name = if name.is_empty() {
        parameter(&content, "name")
    } else {
        name
    };
    if !name.is_empty() || disposition.to_ascii_lowercase().starts_with("attachment") {
        if *counter == index {
            return Ok(Some(AttachmentPart {
                name: if name.is_empty() {
                    "Unnamed attachment".into()
                } else {
                    name
                },
                bytes: transfer_decode(&headers, bytes)?,
                part: if part.is_empty() { "1".into() } else { part.into() },
            }));
        }
        *counter += 1;
        return Ok(None);
    }
    let child = |n: usize| {
        if part.is_empty() {
            n.to_string()
        } else {
            format!("{part}.{n}")
        }
    };
    if kind.starts_with("multipart/") {
        let boundary = parameter(&content, "boundary");
        if boundary.is_empty() {
            return Ok(None);
        }
        for (n, sub) in subparts(bytes, &boundary).0.into_iter().enumerate() {
            // `parse` skips parts it cannot decode; keep the numbering aligned.
            if let Ok(Some(found)) = find_attachment(sub, depth + 1, parts, counter, index, &child(n + 1)) {
                return Ok(Some(found));
            }
        }
        return Ok(None);
    }
    if kind == "message/rfc822" {
        // An embedded message's parts are numbered under its own part.
        let bytes = transfer_decode(&headers, bytes)?;
        return find_attachment(&bytes, depth + 1, parts, counter, index, part);
    }
    Ok(None)
}
fn html_text(text: &str) -> String {
    let doc = parse_html(text, &mut None, InternLiveId::No);
    let mut result = String::new();
    let mut hidden = None;
    for node in doc.nodes {
        match node {
            HtmlNode::OpenTag { lc, .. }
                if lc == id!(script) || lc == id!(style) || lc == id!(head) =>
            {
                if hidden.is_none() {
                    hidden = Some(lc);
                }
            }
            HtmlNode::CloseTag { lc, .. } if hidden == Some(lc) => hidden = None,
            _ if hidden.is_some() => {}
            HtmlNode::Text { start, end, .. } => {
                result.push_str(&doc.decoded[start..end]);
            }
            HtmlNode::OpenTag { lc, .. } | HtmlNode::CloseTag { lc, .. }
                if [
                    id!(br),
                    id!(p),
                    id!(div),
                    id!(tr),
                    id!(li),
                    id!(h1),
                    id!(h2),
                ]
                .contains(&lc) =>
            {
                result.push('\n')
            }
            _ => {}
        }
    }
    result
        .lines()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}
fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|v| v as u8)
}
fn quoted_printable(input: &[u8], word: bool) -> Vec<u8> {
    let mut result = Vec::new();
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'=' {
            if input.get(i + 1) == Some(&b'\n') {
                i += 2;
                continue;
            }
            if input.get(i + 1..i + 3) == Some(b"\r\n") {
                i += 3;
                continue;
            }
            if let (Some(a), Some(b)) = (
                input.get(i + 1).and_then(|b| hex(*b)),
                input.get(i + 2).and_then(|b| hex(*b)),
            ) {
                result.push(a * 16 + b);
                i += 3;
                continue;
            }
        }
        result.push(if word && input[i] == b'_' {
            b' '
        } else {
            input[i]
        });
        i += 1;
    }
    result
}
pub fn decode_words(text: &str) -> String {
    let mut output = String::new();
    let mut rest = text;
    let mut previous = false;
    while let Some(start) = rest.find("=?") {
        let prefix = &rest[..start];
        let encoded = &rest[start + 2..];
        let Some(end) = encoded.find("?=") else { break };
        let mut parts = encoded[..end].splitn(3, '?');
        let charset = parts.next().unwrap_or("");
        let kind = parts.next().unwrap_or("");
        let value = parts.next().unwrap_or("");
        let decoded = if kind.eq_ignore_ascii_case("b") {
            makepad_base64::base64_decode(value.as_bytes()).ok()
        } else if kind.eq_ignore_ascii_case("q") {
            Some(quoted_printable(value.as_bytes(), true))
        } else {
            None
        };
        let Some(bytes) = decoded else {
            output.push_str(&rest[..start + 2]);
            rest = encoded;
            previous = false;
            continue;
        };
        if !(previous && prefix.trim().is_empty()) {
            output.push_str(prefix);
        }
        output.push_str(&decode_charset(&bytes, charset));
        rest = &encoded[end + 2..];
        previous = true;
    }
    output.push_str(rest);
    output
}
fn decode_charset(bytes: &[u8], charset: &str) -> String {
    let charset = charset.trim().to_ascii_lowercase();
    if charset.is_empty() || matches!(charset.as_str(), "utf-8" | "utf8" | "us-ascii") {
        if let Ok(s) = std::str::from_utf8(bytes) {
            return s.to_string();
        }
    }
    #[cfg(target_os = "macos")]
    if let Some(value) = apple_charset(bytes, &charset) {
        return value;
    }
    if matches!(
        charset.as_str(),
        "iso-8859-1" | "latin1" | "windows-1252" | "cp1252" | ""
    ) {
        const WINDOWS: [char; 32] = [
            '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
            '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ',
            '\u{9d}', 'ž', 'Ÿ',
        ];
        return bytes
            .iter()
            .map(|b| {
                if (128..160).contains(b) {
                    WINDOWS[(b - 128) as usize]
                } else {
                    char::from(*b)
                }
            })
            .collect();
    }
    String::from_utf8_lossy(bytes).into_owned()
}
#[cfg(target_os = "macos")]
fn apple_charset(bytes: &[u8], name: &str) -> Option<String> {
    use std::ffi::{c_void, CString};
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFStringCreateWithCString(a: *const c_void, s: *const i8, e: u32) -> *const c_void;
        fn CFStringConvertIANACharSetNameToEncoding(s: *const c_void) -> u32;
        fn CFStringCreateWithBytes(
            a: *const c_void,
            b: *const u8,
            n: isize,
            e: u32,
            x: bool,
        ) -> *const c_void;
        fn CFStringGetLength(s: *const c_void) -> isize;
        fn CFStringGetMaximumSizeForEncoding(n: isize, e: u32) -> isize;
        fn CFStringGetCString(s: *const c_void, b: *mut i8, n: isize, e: u32) -> bool;
        fn CFRelease(s: *const c_void);
    }
    const UTF8: u32 = 0x08000100;
    let name = CString::new(name).ok()?;
    unsafe {
        let cs = CFStringCreateWithCString(std::ptr::null(), name.as_ptr(), UTF8);
        if cs.is_null() {
            return None;
        }
        let encoding = CFStringConvertIANACharSetNameToEncoding(cs);
        CFRelease(cs);
        if encoding == u32::MAX {
            return None;
        }
        let value = CFStringCreateWithBytes(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            encoding,
            false,
        );
        if value.is_null() {
            return None;
        }
        let len = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value), UTF8);
        if !(0..=256 * 1024 * 1024).contains(&len) {
            CFRelease(value);
            return None;
        }
        let mut buffer = vec![0u8; len as usize + 1];
        let ok = CFStringGetCString(
            value,
            buffer.as_mut_ptr().cast(),
            buffer.len() as isize,
            UTF8,
        );
        CFRelease(value);
        if !ok {
            return None;
        }
        buffer.truncate(buffer.iter().position(|b| *b == 0).unwrap_or(buffer.len()));
        String::from_utf8(buffer).ok()
    }
}
fn parse_date(text: &str) -> i64 {
    let text = text.split_once(',').map_or(text, |(_, rest)| rest);
    let parts: Vec<_> = text.split_whitespace().collect();
    if parts.len() < 4 {
        return 0;
    }
    let Some(day) = parts[0].parse::<u32>().ok() else {
        return 0;
    };
    let months = [
        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec",
    ];
    let Some(month) = months.iter().position(|m| parts[1].eq_ignore_ascii_case(m)) else {
        return 0;
    };
    let Some(mut year) = parts[2].parse::<i32>().ok() else {
        return 0;
    };
    if year < 50 {
        year += 2000;
    } else if year < 100 {
        year += 1900;
    }
    if !(1600..=9999).contains(&year)
        || day == 0
        || day > makepad_civil_time::days_in_month(year, month as u32 + 1)
    {
        return 0;
    }
    let time: Vec<u32> = parts[3].split(':').filter_map(|s| s.parse().ok()).collect();
    if time.len() < 2 || time[0] > 23 || time[1] > 59 {
        return 0;
    }
    let zone = parts.get(4).copied().unwrap_or("+0000");
    let offset = match zone.to_ascii_uppercase().as_str() {
        "EST" => -5 * 3600,
        "EDT" => -4 * 3600,
        "CST" => -6 * 3600,
        "CDT" => -5 * 3600,
        "MST" => -7 * 3600,
        "MDT" => -6 * 3600,
        "PST" => -8 * 3600,
        "PDT" => -7 * 3600,
        _ if zone.len() == 5 && zone.as_bytes()[1..].iter().all(u8::is_ascii_digit) => {
            let hh = zone[1..3].parse::<i64>().unwrap_or(0);
            let mm = zone[3..5].parse::<i64>().unwrap_or(0);
            (hh * 3600 + mm * 60) * if zone.starts_with('-') { -1 } else { 1 }
        }
        _ => 0,
    };
    makepad_civil_time::from_ymd(year, month as u32 + 1, day) as i64 * 86400
        + time[0] as i64 * 3600
        + time[1] as i64 * 60
        + time.get(2).copied().unwrap_or(0) as i64
        - offset
}
