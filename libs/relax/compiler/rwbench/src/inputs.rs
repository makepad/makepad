// Decoding real files: inflate (makepad-fast-inflate), the zip reader (makepad-zip-file), PNG
// (makepad-zune-png, the decoder platform/src/app_icon.rs uses) and brotli (the codec
// libs/web_pack uses for the web export). The repo holds no zip/gz/br fixtures, so the compressed
// inputs are made once in setup from real repo files with the repo's own writers.
use crate::fold;
use std::io::Cursor;
use std::path::PathBuf;

pub fn root() -> PathBuf {
    match std::env::var("RW_ROOT") {
        Ok(r) => PathBuf::from(r),
        Err(_) => PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../..")),
    }
}

pub fn read(rel: &str) -> Vec<u8> {
    let p = root().join(rel);
    std::fs::read(&p).unwrap_or_else(|e| panic!("input {}: {}", p.display(), e))
}

/// the sources of makepad-shader: real Rust text, the zip / brotli payload
pub fn script_sources() -> Vec<(String, Vec<u8>)> {
    let dir = root().join("platform/shader/src");
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".rs"))
        .collect();
    names.sort();
    names.into_iter().map(|n| { let d = std::fs::read(dir.join(&n)).unwrap(); (format!("shader/src/{}", n), d) }).collect()
}

fn digest(h: u64, data: &[u8]) -> u64 {
    let mut h = fold(h, data.len() as u64);
    // a sparse but full-coverage digest: every 61st byte plus the length
    let mut i = 0;
    while i < data.len() {
        h = fold(h, data[i] as u64);
        i += 61;
    }
    h
}

pub fn inflate_setup() -> Box<dyn FnMut() -> u64> {
    // a mix as an app loads: a font, source text, markdown
    let mut blobs: Vec<Vec<u8>> = Vec::new();
    blobs.push(read("widgets/resources/IBMPlexSans-Text.ttf"));
    blobs.push(read("AGENTS.md"));
    let mut src = Vec::new();
    for (_, d) in script_sources() {
        src.extend_from_slice(&d);
    }
    blobs.push(src);
    let packed: Vec<(usize, Vec<u8>)> = blobs.iter().map(|b| (b.len(), makepad_fast_inflate::zlib_compress(b, 6))).collect();
    Box::new(move || {
        let mut h = 0u64;
        for (len, z) in &packed {
            let out = makepad_fast_inflate::inflate::decompress_to_vec_zlib(z).unwrap();
            assert_eq!(out.len(), *len);
            h = digest(h, &out);
        }
        h
    })
}

pub fn zip_setup() -> Box<dyn FnMut() -> u64> {
    let mut w = makepad_zip_file::ZipWriter::new();
    for (n, d) in script_sources() {
        w.add(&n, &d, makepad_zip_file::ZipMethod::Deflate).unwrap();
    }
    w.add("fonts/Inter.ttf", &read("widgets/resources/Inter.ttf"), makepad_zip_file::ZipMethod::Deflate).unwrap();
    w.add("fonts/Liberation.ttf", &read("widgets/resources/LiberationMono-Regular.ttf"), makepad_zip_file::ZipMethod::Store).unwrap();
    let zip = w.finish().unwrap();
    Box::new(move || {
        let mut cur = Cursor::new(&zip);
        let dir = makepad_zip_file::zip_read_central_directory(&mut cur).unwrap();
        let mut h = fold(0, dir.file_headers.len() as u64);
        for fh in &dir.file_headers {
            let data = fh.extract(&mut cur).unwrap();
            h = digest(h, &data);
        }
        h
    })
}

pub fn png_setup() -> Box<dyn FnMut() -> u64> {
    use makepad_zune_png::makepad_zune_core::bytestream::ZCursor;
    use makepad_zune_png::makepad_zune_core::options::DecoderOptions;
    use makepad_zune_png::PngDecoder;
    let files = ["apps/files/demos/rubber-duck-illustration.png", "apps/mail/resources/mail-icon.png", "apps/browser/resources/icon_1024.png"];
    let pngs: Vec<Vec<u8>> = files.iter().map(|f| read(f)).collect();
    Box::new(move || {
        let mut h = 0u64;
        for png in &pngs {
            let mut dec = PngDecoder::new_with_options(ZCursor::new(png), DecoderOptions::default());
            dec.decode_headers().unwrap();
            let (w, hh) = dec.dimensions().unwrap();
            let comps = dec.colorspace().unwrap().num_components();
            let out = dec.decode().unwrap();
            let px = out.u8().unwrap();
            assert_eq!(px.len(), w * hh * comps);
            h = fold(h, (w * hh) as u64 ^ comps as u64);
            h = digest(h, &px);
        }
        h
    })
}

pub fn brotli_setup() -> Box<dyn FnMut() -> u64> {
    use std::io::{Read, Write};
    // the web export serves fonts and wasm brotli-compressed (libs/web_pack); quality 5 here only
    // to keep setup short, decoding is what is measured
    let mut blobs: Vec<Vec<u8>> = vec![read("widgets/resources/IBMPlexSans-Text.ttf")];
    let mut src = Vec::new();
    for (_, d) in script_sources() {
        src.extend_from_slice(&d);
    }
    blobs.push(src);
    let packed: Vec<(usize, Vec<u8>)> = blobs
        .iter()
        .map(|b| {
            let mut w = brotli::CompressorWriter::new(Vec::new(), 1 << 16, 5, 22);
            w.write_all(b).unwrap();
            (b.len(), w.into_inner())
        })
        .collect();
    if std::env::var("RW_DEBUG").is_ok() {
        // the compressed streams made in setup: equal under rustc and Relax, or the compressor differs
        for (len, z) in &packed {
            eprintln!("brotli setup: {} -> {} bytes, digest {:x}", len, z.len(), digest(0, z));
        }
    }
    Box::new(move || {
        let mut h = 0u64;
        for (len, z) in &packed {
            let mut out = Vec::with_capacity(*len);
            brotli::Decompressor::new(&z[..], 1 << 16).read_to_end(&mut out).unwrap();
            assert_eq!(out.len(), *len);
            h = digest(h, &out);
        }
        h
    })
}
