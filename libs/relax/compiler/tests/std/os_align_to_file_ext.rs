// <[T]>::align_to / align_to_mut (layout of the three parts for several element/target sizes and
// start offsets) and std::os::unix::fs::FileExt (read_at / write_at / *_exact_at / *_all_at).
// "| " lines must equal real std's. Needed by makepad-fast-inflate (crc32) and the Builder library.
use std::os::unix::fs::FileExt;

#[test]
fn a01_align_to() {
    let bytes: Vec<u8> = (0..64u8).collect();
    for start in 0..9 {
        let s = &bytes[start..start + 40];
        let (l, m, r) = unsafe { s.align_to::<u64>() };
        // pointer alignment varies run to run: print the invariants and the data instead
        let joined: Vec<u8> = l.iter().copied().chain(m.iter().flat_map(|x| x.to_ne_bytes())).chain(r.iter().copied()).collect();
        println!("| u8->u64 start {} parts sum {} mid aligned {} same data {} left<8 {} right<8 {}", start, l.len() + m.len() * 8 + r.len(), m.as_ptr() as usize % 8 == 0, joined == s, l.len() < 8, r.len() < 8);
    }
    let words: Vec<u16> = (0..30u16).collect();
    let (l, m, r) = unsafe { words[1..].align_to::<[u8; 3]>() };
    println!("| u16->[u8;3] {} {} {}", l.len(), m.len(), r.len());
    let (l, m, r) = unsafe { words.align_to::<()>() };
    println!("| zst {} {} {}", l.len(), m.len(), r.len());
    let mut buf = [0u8; 33];
    let (l, m, r) = unsafe { buf.align_to_mut::<u32>() };
    for x in m.iter_mut() {
        *x = 0x01010101;
    }
    let (ll, ml, rl) = (l.len(), m.len(), r.len());
    println!("| mut sum {} ones {}", ll + ml * 4 + rl, buf.iter().filter(|b| **b == 1).count() == ml * 4);
}

#[test]
fn a02_file_ext() {
    let dir = std::env::temp_dir().join(format!("relax-file-ext-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("f");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&p).unwrap();
    f.write_all_at(b"hello world", 0).unwrap();
    println!("| write_at {}", f.write_at(b"WORLD", 6).unwrap());
    let mut b = [0u8; 5];
    println!("| read_at {} {:?}", f.read_at(&mut b, 6).unwrap(), std::str::from_utf8(&b).unwrap());
    f.read_exact_at(&mut b, 0).unwrap();
    println!("| exact {:?}", std::str::from_utf8(&b).unwrap());
    println!("| eof {:?}", f.read_exact_at(&mut b, 9).map_err(|e| e.kind()));
    println!("| past end {}", f.read_at(&mut b, 100).unwrap());
    drop(f);
    std::fs::remove_file(&p).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}

#[test]
fn a03_more_align_to() {
    let empty: [u8; 0] = [];
    let (l, m, r) = unsafe { empty.align_to::<u32>() };
    println!("| empty {} {} {}", l.len(), m.len(), r.len());
    let wide: Vec<u64> = (0..5u64).collect();
    let (l, m, r) = unsafe { wide.align_to::<u8>() };
    println!("| u64->u8 {} {} {} first {:?}", l.len(), m.len(), r.len(), m.first());
    let (l, m, r) = unsafe { wide.align_to::<u32>() };
    println!("| u64->u32 {} {} {}", l.len(), m.len(), r.len());
    let (l, m, r) = unsafe { wide.align_to::<[u8; 3]>() };
    println!("| u64->[u8;3] {} {} {}", l.len(), m.len(), r.len());
    let zst = [(); 7];
    let (l, m, r) = unsafe { zst.align_to::<u8>() };
    println!("| zst src {} {} {}", l.len(), m.len(), r.len());
}

#[test]
fn a04_file_ext_prelude() {
    use std::os::unix::prelude::*;
    let dir = std::env::temp_dir().join(format!("relax-file-ext2-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("g");
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&p).unwrap();
    // writing past the end extends the file with zeros
    f.write_all_at(b"xyz", 10).unwrap();
    let mut b = [9u8; 13];
    f.read_exact_at(&mut b, 0).unwrap();
    println!("| sparse {:?} len {}", b, f.metadata().unwrap().len());
    let mut empty: [u8; 0] = [];
    println!("| zero-length read {} write {}", f.read_at(&mut empty, 5).unwrap(), f.write_at(&[], 5).unwrap());
    drop(f);
    std::fs::remove_file(&p).unwrap();
    std::fs::remove_dir(&dir).unwrap();
}
