//! Git's built-in text conversion between blobs and worktree files.
use crate::oid::ObjectId;
use crate::repo::RepositoryPaths;
use std::path::Path;

/// The config [`cleans_to`] reads: the common one, and the worktree's where the repository uses it.
pub fn conversion_config(p: &RepositoryPaths) -> Result<String, String> {
    let mut config_text = match std::fs::read_to_string(p.common_dir.join("config")) {
        Ok(s) => s, Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(), Err(e) => return Err(e.to_string()),
    };
    if crate::config::get(&config_text, "extensions.worktreeconfig").map_err(|e| e.to_string())?.as_deref().and_then(crate::config::parse_bool) == Some(true) {
        match std::fs::read_to_string(p.git_dir.join("config.worktree")) {
            Ok(extra) => { config_text.push('\n'); config_text.push_str(&extra); }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {},
            Err(e) => return Err(e.to_string()),
        }
    }
    Ok(config_text)
}

/// A path's text attribute, before config supplies the unspecified default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Text { Set, Unset, Auto, #[default] Unspecified }

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Eol { Lf, Crlf, #[default] Unspecified }

/// Built-in conversion and attributes that require an external converter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Conversion {
    pub text: Text,
    pub eol: Eol,
    pub ident: bool,
    pub filter: bool,
    pub working_tree_encoding: bool,
}

/// Read attributes from the caller's tree, then the common dir's info/attributes.
/// The reader takes a repository-relative directory (empty for the root).
pub fn conversion(common: &Path, path: &str, mut read: impl FnMut(&str) -> Result<Option<String>, String>) -> Result<Conversion, String> {
    let mut result = Conversion::default();
    if let Some(rules) = read("")? { apply_attributes(&mut result, &rules, path)?; }
    for (i, _) in path.match_indices('/') {
        if let Some(rules) = read(&path[..i])? { apply_attributes(&mut result, &rules, &path[i + 1..])?; }
    }
    if let Some(rules) = read_attributes(&common.join("info/attributes"))? { apply_attributes(&mut result, &rules, path)?; }
    Ok(result)
}

fn read_attributes(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(Some(s)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

fn apply_attributes(c: &mut Conversion, rules: &str, relative: &str) -> Result<(), String> {
    for line in rules.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
        let mut words = line.split_whitespace();
        let pattern = words.next().unwrap();
        if pattern.starts_with('"') || pattern.starts_with("[attr]") || pattern.starts_with('!') || pattern.contains('\\') {
            return Err("unsupported attribute pattern or macro".into());
        }
        let pattern = pattern.strip_prefix('/').unwrap_or(pattern);
        let name = if pattern.contains('/') { relative } else { relative.rsplit('/').next().unwrap() };
        if !crate::ignore::wildmatch(pattern.as_bytes(), name.as_bytes()) { continue; }
        for word in words {
            match word {
                "binary" | "-text" => c.text = Text::Unset,
                "text" => c.text = Text::Set,
                "text=auto" => c.text = Text::Auto,
                "!text" => c.text = Text::Unspecified,
                "eol=lf" => c.eol = Eol::Lf,
                "eol=crlf" => c.eol = Eol::Crlf,
                "-eol" | "!eol" => c.eol = Eol::Unspecified,
                "ident" => c.ident = true,
                "-ident" | "!ident" => c.ident = false,
                "-filter" | "!filter" => c.filter = false,
                "-working-tree-encoding" | "!working-tree-encoding" => c.working_tree_encoding = false,
                w if w == "filter" || w.starts_with("filter=") => c.filter = true,
                w if w == "working-tree-encoding" || w.starts_with("working-tree-encoding=") => c.working_tree_encoding = true,
                _ => {},
            }
        }
    }
    Ok(())
}

fn supported(c: &Conversion) -> Result<(), String> {
    if c.filter || c.working_tree_encoding {
        return Err("cannot convert a file with filter or working-tree-encoding attributes".into());
    }
    Ok(())
}

fn text_mode(c: &Conversion, config: &str) -> Result<Text, String> {
    if c.text != Text::Unspecified { return Ok(c.text); }
    if c.eol != Eol::Unspecified { return Ok(Text::Set); }
    let auto = crate::config::get(config, "core.autocrlf").map_err(|e| e.to_string())?;
    Ok(if matches!(auto.as_deref(), Some("true" | "input")) { Text::Auto } else { Text::Unspecified })
}

/// Bytes checkout writes, with Git's built-in eol and ident conversion only.
pub fn smudge(raw: &[u8], oid: ObjectId, c: &Conversion, config: &str) -> Result<Vec<u8>, String> {
    supported(c)?;
    let text = text_mode(c, config)?;
    let is_text = text == Text::Set || (text == Text::Auto && text_reader(&mut std::io::Cursor::new(raw)).map_err(|e| e.to_string())?);
    let auto = crate::config::get(config, "core.autocrlf").map_err(|e| e.to_string())?;
    let eol = if c.eol != Eol::Unspecified { c.eol } else {
        match auto.as_deref() {
            Some("true") => Eol::Crlf,
            Some("input") => Eol::Lf,
            _ => match crate::config::get(config, "core.eol").map_err(|e| e.to_string())?.as_deref() {
                Some("lf") => Eol::Lf,
                Some("crlf") => Eol::Crlf,
                _ => if cfg!(windows) { Eol::Crlf } else { Eol::Lf },
            },
        }
    };
    let crlf = is_text && eol == Eol::Crlf && !(text == Text::Auto && raw.contains(&b'\r'));
    let mut out = Vec::with_capacity(raw.len());
    let expanded = format!("$Id: {oid} $");
    let mut at = 0;
    while at < raw.len() {
        if c.ident && raw[at..].starts_with(b"$Id$") {
            out.extend_from_slice(expanded.as_bytes());
            at += 4;
            continue;
        }
        if crlf && raw[at] == b'\n' && (at == 0 || raw[at - 1] != b'\r') { out.push(b'\r'); }
        out.push(raw[at]);
        at += 1;
    }
    Ok(out)
}

/// Whether worktree bytes are this blob, directly or after built-in cleaning.
pub fn bytes_clean_to(bytes: &[u8], oid: ObjectId, c: &Conversion, config: &str) -> Result<bool, String> {
    supported(c)?;
    if crate::oid::hash_object("blob", bytes) == oid { return Ok(true); }
    let normalize = match text_mode(c, config)? {
        Text::Set => true,
        Text::Auto => text_reader(&mut std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?,
        _ => false,
    };
    if !normalize && !c.ident { return Ok(false); }
    let length = canonical_reader(std::io::Cursor::new(bytes), normalize, c.ident, |_| {}).map_err(|e| e.to_string())?;
    let mut hash = crate::Sha1::new();
    hash.update(format!("blob {length}\0").as_bytes());
    canonical_reader(std::io::Cursor::new(bytes), normalize, c.ident, |b| hash.update(b)).map_err(|e| e.to_string())?;
    Ok(ObjectId::from_bytes(hash.finalize()) == oid)
}

/// Whether a changed tracked file cleans to its blob. External converters and
/// unreadable attributes are errors, never evidence that the file is clean.
pub fn cleans_to(root: &Path, common: &Path, path: &str, oid: ObjectId, config: &str) -> Result<bool, String> {
    let c = conversion(common, path, |dir| read_attributes(&root.join(dir).join(".gitattributes")))?;
    supported(&c)?;
    let file = root.join(path);
    let normalize = match text_mode(&c, config)? {
        Text::Set => true,
        Text::Auto => text_bytes(&file).map_err(|e| e.to_string())?,
        _ => false,
    };
    if !normalize && !c.ident { return Ok(false); }
    let length = canonical_bytes(&file, normalize, c.ident, |_| {}).map_err(|e| e.to_string())?;
    let mut hash = crate::Sha1::new();
    hash.update(format!("blob {length}\0").as_bytes());
    canonical_bytes(&file, normalize, c.ident, |bytes| hash.update(bytes)).map_err(|e| e.to_string())?;
    Ok(ObjectId::from_bytes(hash.finalize()) == oid)
}

fn byte(reader: &mut impl std::io::BufRead) -> std::io::Result<Option<u8>> {
    let b = reader.fill_buf()?.first().copied();
    if b.is_some() { reader.consume(1); }
    Ok(b)
}

/// Match Git's text=auto classification without retaining the file.
fn text_bytes(path: &Path) -> std::io::Result<bool> {
    text_reader(&mut std::io::BufReader::new(std::fs::File::open(path)?))
}

fn text_reader(reader: &mut impl std::io::BufRead) -> std::io::Result<bool> {
    let (mut printable, mut nonprintable) = (0u64, 0u64);
    let mut last = None;
    while let Some(b) = byte(reader)? {
        if b == 0 || (last == Some(b'\r') && b != b'\n') { return Ok(false); }
        match b {
            b'\r' | b'\n' => {},
            8 | 9 | 12 | 27 => printable += 1,
            0..=31 | 127 => nonprintable += 1,
            _ => printable += 1,
        }
        last = Some(b);
    }
    if last == Some(26) { nonprintable -= 1; }
    Ok(last != Some(b'\r') && nonprintable <= printable >> 7)
}

/// Two streaming passes supply the blob header length and its hash. Seeking
/// back after a non-Id dollar expression avoids buffering arbitrarily long lines.
fn canonical_bytes(path: &Path, normalize: bool, ident: bool, emit: impl FnMut(&[u8])) -> std::io::Result<u64> {
    canonical_reader(std::io::BufReader::new(std::fs::File::open(path)?), normalize, ident, emit)
}

fn canonical_reader(mut reader: impl std::io::BufRead + std::io::Seek, normalize: bool, ident: bool, mut emit: impl FnMut(&[u8])) -> std::io::Result<u64> {
    use std::io::SeekFrom;
    let mut block = [0u8; 8192];
    let (mut used, mut total) = (0, 0u64);
    let mut push = |bytes: &[u8]| {
        for b in bytes {
            block[used] = *b; used += 1; total += 1;
            if used == block.len() { emit(&block); used = 0; }
        }
    };
    while let Some(b) = byte(&mut reader)? {
        if normalize && b == b'\r' && reader.fill_buf()?.first() == Some(&b'\n') { continue; }
        if ident && b == b'$' {
            let start = reader.stream_position()?;
            let mut prefix = [0; 3];
            let mut matched = false;
            if reader.read_exact(&mut prefix).is_ok() && prefix == *b"Id:" {
                while let Some(end) = byte(&mut reader)? {
                    if end == b'$' { matched = true; break; }
                    if end == b'\n' { break; }
                }
            }
            if matched { push(b"$Id$"); continue; }
            reader.seek(SeekFrom::Start(start))?;
        }
        push(&[b]);
    }
    drop(push);
    emit(&block[..used]);
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::oid::hash_object;

    #[test]
    fn checkout_conversion_and_verification() {
        let dir = crate::test_support::tempdir().unwrap();
        let mut read_dirs = Vec::new();
        let crlf = conversion(dir.path(), "scripts/run.bat", |base| {
            read_dirs.push(base.to_string());
            Ok((base == "").then(|| "*.bat text eol=crlf".into()))
        }).unwrap();
        assert_eq!(read_dirs, ["", "scripts"], "attributes come from each directory of the supplied tree");
        assert!(!dir.path().join(".gitattributes").exists(), "no on-disk attribute is needed");
        assert_eq!((crlf.text, crlf.eol), (Text::Set, Eol::Crlf), "the eol value is retained");
        for (raw, c, config, expected) in [
            (&b"a\nb\n"[..], crlf, "", &b"a\r\nb\r\n"[..]),
            (b"a\r\nb\n", crlf, "", b"a\r\nb\r\n"),
            (b"a\r\nb\n", Conversion { text: Text::Auto, eol: Eol::Crlf, ..Default::default() }, "", b"a\r\nb\n"),
            (b"a\0\nb", Conversion { text: Text::Auto, eol: Eol::Crlf, ..Default::default() }, "", b"a\0\nb"),
            (b"a\nb\n", Conversion { eol: Eol::Lf, ..Default::default() }, "", b"a\nb\n"),
            (b"a\nb\n", Conversion::default(), "[core]\n autocrlf = true\n", b"a\r\nb\r\n"),
            (b"a\nb\n", Conversion::default(), "[core]\n autocrlf = input\n eol = crlf\n", b"a\nb\n"),
        ] {
            let oid = hash_object("blob", raw);
            assert_eq!(smudge(raw, oid, &c, config).unwrap(), expected, "checkout eol for {raw:?} under {c:?}");
        }
        let c = Conversion { ident: true, ..crlf };
        let raw = b"a\n$Id$\n";
        let oid = hash_object("blob", raw);
        let out = smudge(raw, oid, &c, "").unwrap();
        assert_eq!(out, format!("a\r\n$Id: {oid} $\r\n").as_bytes(), "ident expands the blob id");
        assert!(bytes_clean_to(&out, oid, &c, "").unwrap(), "smudged bytes clean to their blob");
        assert!(!bytes_clean_to(&out[..3], oid, &c, "").unwrap(), "a short copy is rejected");
        for c in [Conversion { filter: true, ..c }, Conversion { working_tree_encoding: true, ..c }] {
            assert!(smudge(raw, oid, &c, "").is_err(), "external conversions cannot be written");
        }
        assert!(conversion(dir.path(), "x", |_| Ok(Some("[attr]custom text".into()))).is_err(), "macros are refused");
    }
}
