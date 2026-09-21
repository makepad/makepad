//! rustc crate-metadata header reader/patcher (rustc 1.98 rmeta, METADATA_VERSION 10).
//!
//! Why the Android super-app needs it: every crate's metadata records the SVH
//! (crate hash) of each proc-macro it loaded, and rustc refuses a proc-macro
//! dylib whose header carries another SVH — as E0463 "can't find crate for X
//! which Y depends on", because the proc-macro search runs on a throwaway
//! locator whose rejections are never reported. The SVH hashes the compiler
//! options (host target triple, cfg) and the upstream std hashes, so a
//! proc-macro rebuilt by the phone's musl-host rustc never carries the SVH the
//! Mac-built engine recorded. The phone therefore rewrites the header SVH of
//! each rebuilt proc-macro `.so` to the recorded one
//! (`assets/wmdyn/proc-macro-svh.txt`, written by local/wm-dyn/pack.py with
//! the same decoder, local/wm-dyn/rmeta.py). Nothing re-derives an SVH: rustc
//! compares it for identity and copies it into dependents.
//!
//! Layout (rustc_metadata/src/rmeta/mod.rs): a dylib's `.rustc` section is
//! METADATA_HEADER (`rust\0\0\0` + version byte) + u64 LE blob length + blob;
//! an rlib's `lib.rmeta` member is the blob. Blob: METADATA_HEADER + u64 LE
//! root position + String rustc version; at root, `CrateRoot { header:
//! CrateHeader { triple: TargetTuple, hash: Svh, name: Symbol,
//! is_proc_macro_crate, is_stub }, extra_filename, … }`. Integers are
//! LEB128, a String is len + bytes + 0xC1, the Svh is 16 raw bytes.

/// `METADATA_HEADER` without its version byte.
const MAGIC: &[u8; 7] = b"rust\0\0\0";
const STR_SENTINEL: u8 = 0xC1;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: String,
    pub triple: String,
    pub name: String,
    pub svh: [u8; 16],
    /// Byte offset of the 16 SVH bytes in the file.
    pub svh_offset: usize,
    pub proc_macro: bool,
}

fn leb(b: &[u8], mut p: usize) -> Result<(u64, usize), String> {
    let (mut v, mut shift) = (0u64, 0u32);
    loop {
        let x = *b.get(p).ok_or("leb128 past end")?;
        p += 1;
        if shift >= 64 {
            return Err("leb128 too long".into());
        }
        v |= u64::from(x & 0x7F) << shift;
        if x < 0x80 {
            return Ok((v, p));
        }
        shift += 7;
    }
}

fn string(b: &[u8], p: usize) -> Result<(String, usize), String> {
    let (n, p) = leb(b, p)?;
    let n = usize::try_from(n).map_err(|_| "string length")?;
    let bytes = b.get(p..p + n).ok_or("string past end")?;
    let s = std::str::from_utf8(bytes).map_err(|_| "string not utf-8")?.to_string();
    if b.get(p + n) != Some(&STR_SENTINEL) {
        return Err("string sentinel missing".into());
    }
    Ok((s, p + n + 1))
}

fn symbol(b: &[u8], p: usize) -> Result<(String, usize), String> {
    match *b.get(p).ok_or("symbol past end")? {
        0 => string(b, p + 1),
        1 => {
            let (off, p) = leb(b, p + 1)?;
            Ok((string(b, usize::try_from(off).map_err(|_| "symbol offset")?)?.0, p))
        }
        2 => {
            let (idx, p) = leb(b, p + 1)?;
            Ok((format!("<predefined {idx}>"), p))
        }
        t => Err(format!("bad symbol tag {t}")),
    }
}

fn u64_at(b: &[u8], p: usize) -> Result<u64, String> {
    let x: [u8; 8] = b.get(p..p + 8).ok_or("u64 past end")?.try_into().unwrap();
    Ok(u64::from_le_bytes(x))
}

/// Decode a blob that starts at `start` (its own METADATA_HEADER first).
fn header_at(data: &[u8], start: usize, len: usize) -> Result<Header, String> {
    let b = data.get(start..start + len).ok_or("blob past end")?;
    if b.get(..7) != Some(&MAGIC[..]) {
        return Err("blob magic".into());
    }
    let root = usize::try_from(u64_at(b, 8)?).map_err(|_| "root position")?;
    let (version, _) = string(b, 16)?;
    if !version.starts_with("rustc ") {
        return Err(format!("not a rustc version string: {version:?}"));
    }
    let (variant, p) = leb(b, root)?;
    if variant != 0 {
        return Err(format!("TargetTuple variant {variant}"));
    }
    let (triple, p) = string(b, p)?;
    let svh: [u8; 16] = b.get(p..p + 16).ok_or("svh past end")?.try_into().unwrap();
    let (name, q) = symbol(b, p + 16)?;
    let proc_macro = *b.get(q).ok_or("flags past end")? != 0;
    Ok(Header { version, triple, name, svh, svh_offset: start + p, proc_macro })
}

/// The crate header of an rlib, dylib or proc-macro `.so`/`.dylib` image.
pub fn read_header(data: &[u8]) -> Result<Header, String> {
    let mut last = String::from("no rustc metadata");
    let mut i = 0;
    while let Some(k) = data[i..].windows(MAGIC.len()).position(|w| w == MAGIC) {
        let at = i + k;
        i = at + 1;
        // dylib section: header + u64 length + blob (which repeats the header)
        if data.get(at + 16..at + 23) == Some(&MAGIC[..]) {
            if let Ok(n) = u64_at(data, at + 8).and_then(|n| usize::try_from(n).map_err(|_| "len".to_string())) {
                match header_at(data, at + 16, n) {
                    Ok(h) => return Ok(h),
                    Err(e) => {
                        last = e;
                        continue;
                    }
                }
            }
        }
        match header_at(data, at, data.len() - at) {
            Ok(h) => return Ok(h),
            Err(e) => last = e,
        }
    }
    Err(last)
}

/// Overwrite the header SVH in place (the image `header` was read from).
pub fn set_svh(data: &mut [u8], header: &Header, svh: [u8; 16]) {
    data[header.svh_offset..header.svh_offset + 16].copy_from_slice(&svh);
}

pub fn svh_hex(svh: &[u8; 16]) -> String {
    svh.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn parse_svh(hex: &str) -> Option<[u8; 16]> {
    if hex.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        *o = u8::from_str_radix(&hex[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn enc_leb(mut v: u64, out: &mut Vec<u8>) {
        loop {
            let byte = (v & 0x7F) as u8;
            v >>= 7;
            if v == 0 {
                out.push(byte);
                return;
            }
            out.push(byte | 0x80);
        }
    }

    fn enc_str(s: &str, out: &mut Vec<u8>) {
        enc_leb(s.len() as u64, out);
        out.extend_from_slice(s.as_bytes());
        out.push(STR_SENTINEL);
    }

    /// A blob shaped like rustc's: header, root position, version, padding, root.
    fn blob(svh: [u8; 16], name_by_offset: bool) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(MAGIC);
        b.push(10);
        b.extend_from_slice(&[0; 8]);
        enc_str("rustc 1.98.0 (88d9e12ae 2026-08-18)", &mut b);
        let name_at = b.len();
        enc_str("pm_crate", &mut b); // a symbol table entry a later SYMBOL_OFFSET may point at
        b.extend_from_slice(&[0xEE; 37]);
        let root = b.len() as u64;
        enc_leb(0, &mut b); // TargetTuple::TargetTuple
        enc_str("aarch64-unknown-linux-musl", &mut b);
        b.extend_from_slice(&svh);
        if name_by_offset {
            b.push(1);
            enc_leb(name_at as u64, &mut b);
        } else {
            b.push(0);
            enc_str("pm_crate", &mut b);
        }
        b.push(1); // is_proc_macro_crate
        b.push(0); // is_stub
        enc_str("-f2156a3299472f74", &mut b);
        b[8..16].copy_from_slice(&root.to_le_bytes());
        b
    }

    fn dylib_image(blob: &[u8]) -> Vec<u8> {
        let mut d = b"\x7fELF junk rust\0\0\0 more junk rust\0\0\0\x0a\x01\x00".to_vec(); // decoys
        d.extend_from_slice(MAGIC);
        d.push(10);
        d.extend_from_slice(&(blob.len() as u64).to_le_bytes());
        d.extend_from_slice(blob);
        d.extend_from_slice(b"trailing section data");
        d
    }

    #[test]
    fn reads_dylib_section_and_rlib_blob() {
        let svh = [7u8; 16];
        for by_offset in [false, true] {
            let blob = blob(svh, by_offset);
            let rlib = read_header(&blob).unwrap();
            assert_eq!(rlib.name, "pm_crate");
            assert_eq!(rlib.triple, "aarch64-unknown-linux-musl");
            assert_eq!(rlib.svh, svh);
            assert!(rlib.proc_macro);
            assert_eq!(&blob[rlib.svh_offset..rlib.svh_offset + 16], &svh);
            let image = dylib_image(&blob);
            let dylib = read_header(&image).unwrap();
            assert_eq!(dylib.name, "pm_crate");
            assert_eq!(&image[dylib.svh_offset..dylib.svh_offset + 16], &svh);
            assert!(dylib.svh_offset > rlib.svh_offset);
        }
    }

    #[test]
    fn patches_in_place() {
        let mut image = dylib_image(&blob([1u8; 16], false));
        let h = read_header(&image).unwrap();
        let want = parse_svh("a28005c42d96614184ca82ff91af562e").unwrap();
        set_svh(&mut image, &h, want);
        let again = read_header(&image).unwrap();
        assert_eq!(again.svh, want);
        assert_eq!(svh_hex(&again.svh), "a28005c42d96614184ca82ff91af562e");
        assert_eq!(again.svh_offset, h.svh_offset);
        assert!(parse_svh("zz").is_none());
    }

    #[test]
    fn rejects_garbage() {
        assert!(read_header(b"nothing here").is_err());
        assert!(read_header(b"rust\0\0\0\x0a\xff\xff\xff\xff\xff\xff\xff\xff").is_err());
    }

    /// The real thing when the pack lives next to the checkout (Mac only).
    #[test]
    fn reads_a_real_proc_macro_when_present() {
        let Ok(manifest_dir) = std::env::var("CARGO_MANIFEST_DIR") else { return };
        let dir = format!("{manifest_dir}/../../local/wm-dyn/stage/target/release/deps");
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let n = e.file_name().to_string_lossy().to_string();
            if n.starts_with("libmakepad_micro_serde_derive-") && n.ends_with(".dylib") {
                let data = std::fs::read(e.path()).unwrap();
                let h = read_header(&data).unwrap();
                assert_eq!(h.name, "makepad_micro_serde_derive");
                assert_eq!(h.triple, "aarch64-apple-darwin");
                assert!(h.proc_macro);
                assert!(h.version.starts_with("rustc 1."));
            }
        }
    }
}
