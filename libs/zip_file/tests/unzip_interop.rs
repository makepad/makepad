//! Interop: archives we write must satisfy a real zip implementation, not just
//! our own reader. Skips (rather than fails) where `unzip` is absent.

use makepad_zip_file::{ZipMethod, ZipWriter};
use std::process::Command;

fn have_unzip() -> bool {
    Command::new("unzip")
        .arg("-v")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn write_archive(path: &std::path::Path, method: ZipMethod) {
    let mut w = ZipWriter::new();
    w.add("game.splash", b"game.box({pos: vec3(0,1,0)})\n", method)
        .unwrap();
    w.add("manifest.toml", b"name = \"demo\"\nplayers_max = 4\n", method)
        .unwrap();
    w.add("assets/blob.bin", &vec![7u8; 50_000], method).unwrap();
    w.add("nested/dir/deep.txt", b"deep", method).unwrap();
    std::fs::write(path, w.finish().unwrap()).unwrap();
}

fn tmp_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("makepad-zip-interop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn system_unzip_validates_our_archives() {
    if !have_unzip() {
        eprintln!("skipping: `unzip` not on PATH");
        return;
    }
    for (tag, method) in [("store", ZipMethod::Store), ("deflate", ZipMethod::Deflate)] {
        let dir = tmp_dir(tag);
        let zip = dir.join("pkg.arcade");
        write_archive(&zip, method);

        // -t verifies every member's CRC against its decompressed bytes, which
        // is what catches a malformed central directory or a wrong crc/size.
        let out = Command::new("unzip").arg("-t").arg(&zip).output().unwrap();
        assert!(
            out.status.success(),
            "unzip -t failed for {tag}:\n{}\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );

        // And the extracted bytes must actually match.
        let dest = dir.join("out");
        std::fs::create_dir_all(&dest).unwrap();
        let out = Command::new("unzip")
            .arg("-q")
            .arg(&zip)
            .arg("-d")
            .arg(&dest)
            .output()
            .unwrap();
        assert!(out.status.success(), "unzip extract failed for {tag}");
        assert_eq!(
            std::fs::read(dest.join("game.splash")).unwrap(),
            b"game.box({pos: vec3(0,1,0)})\n"
        );
        assert_eq!(
            std::fs::read(dest.join("assets/blob.bin")).unwrap(),
            vec![7u8; 50_000]
        );
        assert_eq!(
            std::fs::read(dest.join("nested/dir/deep.txt")).unwrap(),
            b"deep"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
