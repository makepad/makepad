// Benchmark: decompress all objects in a real git pack file
//
// Run with: cargo bench -p makepad-fast-inflate --bench pack_bench

use std::time::Instant;

fn find_pack_file() -> Option<std::path::PathBuf> {
    // Walk up from cwd to find .git/objects/pack/*.pack
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let pack_dir = dir.join(".git/objects/pack");
        if pack_dir.is_dir() {
            let mut largest: Option<(u64, std::path::PathBuf)> = None;
            for entry in std::fs::read_dir(&pack_dir).ok()? {
                let entry = entry.ok()?;
                let path = entry.path();
                if path.extension().map_or(false, |e| e == "pack") {
                    let size = entry.metadata().ok()?.len();
                    if largest.as_ref().map_or(true, |(s, _)| size > *s) {
                        largest = Some((size, path));
                    }
                }
            }
            return largest.map(|(_, p)| p);
        }
        if !dir.pop() {
            return None;
        }
    }
}

struct PackObject {
    compressed_data: Vec<u8>,
    expected_size: usize,
}

fn parse_pack_objects(pack_data: &[u8]) -> Vec<PackObject> {
    // Minimal pack parser: just extract compressed chunks and expected sizes
    let mut objects = Vec::new();

    if pack_data.len() < 12 {
        return objects;
    }
    // Header: "PACK" + version(4) + num_objects(4)
    if &pack_data[0..4] != b"PACK" {
        return objects;
    }
    let num_objects =
        u32::from_be_bytes([pack_data[8], pack_data[9], pack_data[10], pack_data[11]]) as usize;
    let mut pos = 12;

    for _ in 0..num_objects {
        if pos >= pack_data.len() {
            break;
        }

        // Read object header (variable-length encoding)
        let mut byte = pack_data[pos];
        let obj_type = (byte >> 4) & 0x7;
        let mut size = (byte & 0x0F) as usize;
        let mut shift = 4;
        pos += 1;

        while byte & 0x80 != 0 {
            if pos >= pack_data.len() {
                return objects;
            }
            byte = pack_data[pos];
            size |= ((byte & 0x7F) as usize) << shift;
            shift += 7;
            pos += 1;
        }

        // For delta objects (6=ofs_delta, 7=ref_delta), skip the base reference
        if obj_type == 6 {
            // OFS_DELTA: variable-length negative offset
            if pos >= pack_data.len() {
                break;
            }
            byte = pack_data[pos];
            pos += 1;
            while byte & 0x80 != 0 {
                if pos >= pack_data.len() {
                    return objects;
                }
                byte = pack_data[pos];
                pos += 1;
            }
        } else if obj_type == 7 {
            // REF_DELTA: 20-byte base object SHA
            pos += 20;
            if pos > pack_data.len() {
                break;
            }
        }

        // The rest is zlib-compressed data. We need to figure out how much
        // compressed data there is. Use the consumed byte count to find the boundary.
        let remaining = &pack_data[pos..];
        if remaining.len() < 2 {
            break;
        }

        // Try to decompress to find how many compressed bytes were consumed
        let mut decompressed = vec![0; size];
        if let Ok((consumed, written)) =
            makepad_fast_inflate::zlib_decompress(remaining, &mut decompressed)
        {
            assert_eq!(written, size);
            objects.push(PackObject {
                compressed_data: remaining[..consumed].to_vec(),
                expected_size: decompressed.len(),
            });
            pos += consumed;
        } else {
            // Can't parse further
            break;
        }
    }

    objects
}

fn main() {
    let pack_path = match find_pack_file() {
        Some(p) => p,
        None => {
            println!("No .git pack file found, skipping benchmark");
            return;
        }
    };

    println!(
        "Pack file: {} ({:.1} MB)",
        pack_path.display(),
        std::fs::metadata(&pack_path).unwrap().len() as f64 / 1e6
    );

    let pack_data = std::fs::read(&pack_path).unwrap();
    println!("Parsing pack objects...");
    let objects = parse_pack_objects(&pack_data);
    println!("Found {} objects", objects.len());

    let total_compressed: usize = objects.iter().map(|o| o.compressed_data.len()).sum();
    let total_decompressed: usize = objects.iter().map(|o| o.expected_size).sum();
    println!(
        "Total compressed: {:.1} MB, decompressed: {:.1} MB",
        total_compressed as f64 / 1e6,
        total_decompressed as f64 / 1e6
    );
    println!();

    let iterations = 3;

    // --- fast_inflate ---
    println!("fast_inflate (Rust, with intrinsics):");
    let mut best_rust = f64::MAX;
    for i in 0..iterations {
        let start = Instant::now();
        let mut total_written = 0usize;
        for obj in &objects {
            let mut out = vec![0u8; obj.expected_size];
            let (_, written) =
                makepad_fast_inflate::zlib_decompress(&obj.compressed_data, &mut out)
                    .expect("fast_inflate failed");
            total_written += written;
        }
        let elapsed = start.elapsed().as_secs_f64();
        let throughput = total_decompressed as f64 / elapsed / 1e6;
        println!(
            "  run {}: {:.3}s  ({:.1} MB/s decompressed, {} bytes written)",
            i + 1,
            elapsed,
            throughput,
            total_written
        );
        best_rust = best_rust.min(elapsed);
    }

    println!();

    println!("Best: {:.3}s ({:.1} MB/s)", best_rust, total_decompressed as f64 / best_rust / 1e6);
}
