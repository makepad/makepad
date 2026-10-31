// The compositor is Linux-only (the crate's modules are cfg(target_os = "linux")).
#![cfg(target_os = "linux")]

use std::io::Write;
use std::os::fd::OwnedFd;
use wayland_server::shm::*;

fn tmpfile(tag: &str, len: u64) -> (std::fs::File, std::path::PathBuf) {
    let p = std::env::temp_dir().join(format!("wsrv-shm-{}-{tag}", std::process::id()));
    let f = std::fs::File::options().read(true).write(true).create(true).truncate(true).open(&p).unwrap();
    f.set_len(len).unwrap();
    (f, p)
}

#[test]
fn reads_and_validates() {
    let (mut f, p) = tmpfile("a", 8192);
    let data: Vec<u8> = (0..8192u32).map(|i| (i % 251) as u8).collect();
    f.write_all(&data).unwrap();
    let pool = Pool::new(OwnedFd::from(f.try_clone().unwrap()), 8192).unwrap();
    let b = BufferSpec::new(pool.len(), 16, 10, 4, 48, ARGB8888).unwrap();
    let mut out = Vec::new();
    copy_rows(&pool, &b, 1, 3, &mut out).unwrap();
    assert_eq!(out.len(), 80);
    assert_eq!(&out[..40], &data[16 + 48..16 + 48 + 40]);
    assert_eq!(&out[40..], &data[16 + 96..16 + 96 + 40]);
    // invalid specs
    assert!(BufferSpec::new(8192, -1, 10, 4, 48, ARGB8888).is_err());
    assert!(BufferSpec::new(8192, 0, 10, 4, 39, ARGB8888).is_err());
    assert!(BufferSpec::new(8192, 8100, 10, 4, 48, ARGB8888).is_err());
    assert!(BufferSpec::new(8192, 0, 10, 4, 48, 77).is_err());
    assert!(BufferSpec::new(usize::MAX, 0, i32::MAX, i32::MAX, i32::MAX, ARGB8888).is_err());
    // pool bigger than the file is refused
    assert_eq!(Pool::new(OwnedFd::from(f.try_clone().unwrap()), 16384).err(), Some(ShmError::TooSmall));
    std::fs::remove_file(p).unwrap();
}

#[test]
fn truncation_does_not_crash() {
    let (f, p) = tmpfile("b", 3 * PAGE as u64);
    let pool = Pool::new(OwnedFd::from(f.try_clone().unwrap()), 3 * PAGE).unwrap();
    f.set_len(PAGE as u64).unwrap(); // client shrinks the file
    let mut out = vec![1u8; 3 * PAGE];
    assert_eq!(pool.read(0, &mut out), Err(ShmError::Truncated));
    assert!(out[PAGE..].iter().all(|&b| b == 0)); // lost pages read as zeroes
    // the guard is off again afterwards and a second fault in another page is handled too
    let (f2, p2) = tmpfile("c", 2 * PAGE as u64);
    let pool2 = Pool::new(OwnedFd::from(f2.try_clone().unwrap()), 2 * PAGE).unwrap();
    f2.set_len(0).unwrap();
    let mut o2 = vec![0u8; 2 * PAGE];
    assert_eq!(pool2.read(0, &mut o2), Err(ShmError::Truncated));
    // faults from several threads at once
    let handles: Vec<_> = (0..4).map(|i| {
        let (f, p) = tmpfile(&format!("t{i}"), 2 * PAGE as u64);
        let pool = Pool::new(OwnedFd::from(f.try_clone().unwrap()), 2 * PAGE).unwrap();
        f.set_len(0).unwrap();
        std::thread::spawn(move || {
            let mut o = vec![0u8; 2 * PAGE];
            for _ in 0..50 { let _ = &f; assert_eq!(pool.read(0, &mut o), Err(ShmError::Truncated)); }
            std::fs::remove_file(p).unwrap();
        })
    }).collect();
    for h in handles { h.join().unwrap(); }
    std::fs::remove_file(p).unwrap();
    std::fs::remove_file(p2).unwrap();
}

#[test]
fn resize_grows() {
    let (f, p) = tmpfile("d", PAGE as u64);
    let mut pool = Pool::new(OwnedFd::from(f.try_clone().unwrap()), PAGE).unwrap();
    assert!(pool.resize(PAGE / 2).is_err());
    f.set_len(2 * PAGE as u64).unwrap();
    pool.resize(2 * PAGE).unwrap();
    let mut o = [0u8; 8];
    pool.read(2 * PAGE - 8, &mut o).unwrap();
    assert_eq!(pool.read(2 * PAGE - 4, &mut o), Err(ShmError::BadBuffer));
    std::fs::remove_file(p).unwrap();
}
