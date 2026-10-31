//! LZX against real Microsoft cabinets, byte for byte.
//!
//! The cabinets are not in the repository (they are Microsoft's files). Put
//! LZX cabinets (the Visual Studio packages' `cab1.cab` payloads, for example)
//! in a folder, extract each `<name>.cab` with an independent tool into
//! `<refs>/<name>/` (`bsdtar -xf <name>.cab -C <refs>/<name>`), and run
//!
//!   LZX_CAB_DIR=<folder> LZX_REF_DIR=<refs> cargo test --release -p makepad-loader \
//!       --test lzx_real_cabs -- --ignored --nocapture
//!
//! Every file of every cabinet with a reference folder is compared, the
//! others are only decoded; the decompression throughput is printed
//! (`LZX_ROUNDS=n` repeats it).

// A native test: the wasm clock rule does not apply.
#![allow(clippy::disallowed_types, clippy::disallowed_methods)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

#[test]
#[ignore]
fn lzx_matches_reference_output() {
    let Ok(cab_dir) = std::env::var("LZX_CAB_DIR") else {
        eprintln!("LZX_CAB_DIR not set; skipped");
        return;
    };
    let ref_dir = PathBuf::from(std::env::var("LZX_REF_DIR").expect("LZX_REF_DIR"));
    let rounds: usize = std::env::var("LZX_ROUNDS").ok().and_then(|v| v.parse().ok()).unwrap_or(1);
    let mut names: Vec<_> = std::fs::read_dir(&cab_dir)
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("cab")))
        .collect();
    names.sort();
    let (mut total, mut elapsed, mut cabs, mut files, mut failures) = (0u64, Duration::ZERO, 0usize, 0usize, Vec::new());
    let mut unreferenced = 0usize;
    for path in &names {
        let stem = path.file_stem().unwrap().to_string_lossy().into_owned();
        let refs = ref_dir.join(&stem);
        // Without a reference the cabinet is only decoded (and must not fail).
        let has_refs = std::fs::read_dir(&refs).is_ok_and(|mut d| d.next().is_some());
        let data = std::fs::read(path).unwrap();
        let parsed = makepad_loader::cab::parse(&data).unwrap();
        let mut outs = Vec::new();
        for (index, folder) in parsed.folders.iter().enumerate() {
            let mut out = Err(String::new());
            let start = Instant::now();
            for _ in 0..rounds {
                out = makepad_loader::cab::decompress_folder(&data, folder);
            }
            elapsed += start.elapsed();
            match out {
                Ok(out) => {
                    total += (out.len() * rounds) as u64;
                    outs.push(Some(out));
                }
                Err(e) => {
                    failures.push(format!("{stem} folder {index}: {e}"));
                    outs.push(None);
                }
            }
        }
        cabs += 1;
        if !has_refs {
            if let Ok(dump) = std::env::var("LZX_DUMP_DIR") {
                for f in &parsed.files {
                    if let Some(Some(buf)) = outs.get(f.folder) {
                        let p = PathBuf::from(&dump).join(&stem).join(f.name.replace('\\', "/"));
                        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
                        std::fs::write(p, &buf[f.offset..f.offset + f.size]).unwrap();
                    }
                }
            }
            unreferenced += 1;
            continue;
        }
        for f in &parsed.files {
            let Some(Some(buf)) = outs.get(f.folder) else { continue };
            let got = &buf[f.offset..f.offset + f.size];
            let expected = std::fs::read(refs.join(f.name.replace('\\', "/"))).unwrap_or_else(|e| panic!("{stem}/{}: {e}", f.name));
            files += 1;
            if expected != got {
                let at = expected.iter().zip(got).position(|(a, b)| a != b).unwrap_or(expected.len().min(got.len()));
                failures.push(format!("{stem}/{}: differs at byte {at} ({} vs {} bytes)", f.name, got.len(), expected.len()));
            }
        }
    }
    let secs = elapsed.as_secs_f64();
    println!(
        "lzx: {cabs} cabinets ({unreferenced} without a reference, decoded only), {files} files compared, {:.1} MB out in {secs:.3} s = {:.1} MB/s",
        total as f64 / 1e6,
        total as f64 / 1e6 / secs,
    );
    assert!(failures.is_empty(), "{} failed:\n{}", failures.len(), failures.join("\n"));
}
