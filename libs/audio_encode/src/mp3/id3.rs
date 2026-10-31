//! ID3v2.3 tag writer: the text frames players show (title, artist, album,
//! genre, year, comment) and unsynchronised lyrics (USLT). Version 2.3 with
//! UTF-16 text is what every phone, chat app and desktop player reads.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Id3Tags {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub genre: Option<String>,
    pub year: Option<String>,
    pub track: Option<String>,
    pub comment: Option<String>,
    pub lyrics: Option<String>,
    /// ISO-639-2 language of the lyrics; `None` writes "und".
    pub lyrics_language: Option<String>,
}

/// UTF-16 with a byte-order mark (ID3v2.3 text encoding 1).
fn utf16(s: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(&[0xff, 0xfe]);
    for u in s.encode_utf16() {
        out.extend_from_slice(&u.to_le_bytes());
    }
}

fn frame(id: &[u8; 4], body: &[u8], out: &mut Vec<u8>) {
    out.extend_from_slice(id);
    out.extend_from_slice(&(body.len() as u32).to_be_bytes());
    out.extend_from_slice(&[0, 0]);
    out.extend_from_slice(body);
}

fn text_frame(id: &[u8; 4], value: &Option<String>, out: &mut Vec<u8>) {
    let Some(v) = value.as_deref().map(str::trim).filter(|v| !v.is_empty()) else {
        return;
    };
    let mut body = vec![1u8];
    utf16(v, &mut body);
    frame(id, &body, out);
}

/// `COMM` / `USLT` share a layout: encoding, language, a descriptor, the text.
fn language_frame(id: &[u8; 4], language: &str, value: &Option<String>, out: &mut Vec<u8>) {
    let Some(v) = value.as_deref().filter(|v| !v.trim().is_empty()) else {
        return;
    };
    let mut body = vec![1u8];
    let lang = language.trim().to_ascii_lowercase();
    let lang = if lang.len() == 3 && lang.bytes().all(|b| b.is_ascii_lowercase()) { lang } else { "und".to_string() };
    body.extend_from_slice(lang.as_bytes());
    utf16("", &mut body);
    body.extend_from_slice(&[0, 0]);
    utf16(v, &mut body);
    frame(id, &body, out);
}

/// The whole tag, header included; empty when no field is set.
pub fn id3v2(tags: &Id3Tags) -> Vec<u8> {
    let mut frames = Vec::new();
    text_frame(b"TIT2", &tags.title, &mut frames);
    text_frame(b"TPE1", &tags.artist, &mut frames);
    text_frame(b"TALB", &tags.album, &mut frames);
    text_frame(b"TCON", &tags.genre, &mut frames);
    text_frame(b"TYER", &tags.year, &mut frames);
    text_frame(b"TRCK", &tags.track, &mut frames);
    language_frame(b"COMM", "und", &tags.comment, &mut frames);
    let lang = tags.lyrics_language.as_deref().unwrap_or("und");
    language_frame(b"USLT", lang, &tags.lyrics, &mut frames);
    if frames.is_empty() {
        return frames;
    }
    let size = frames.len() as u32;
    let mut out = Vec::with_capacity(frames.len() + 10);
    out.extend_from_slice(b"ID3\x03\x00\x00");
    // syncsafe size: 7 bits per byte
    out.extend_from_slice(&[(size >> 21) as u8 & 0x7f, (size >> 14) as u8 & 0x7f, (size >> 7) as u8 & 0x7f, size as u8 & 0x7f]);
    out.extend_from_slice(&frames);
    out
}
