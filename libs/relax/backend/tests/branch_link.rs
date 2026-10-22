//! Branch relocations (calls.DESIGN.md): `call rel32` / `bl` fields linked directly in reach,
//! through a veneer out of reach; thunk heads.
use relax_backend::rir::{branch_reaches, link, link_with, Reloc, RelocAt, RelocKind};

#[test]
fn x64_call_in_reach_and_through_veneer() {
    // call rel32 at 0x1000 (disp32 at +1)
    let base = 0x1000u64;
    let r = [RelocAt { at: 1, kind: RelocKind::X64Branch32, target: Reloc::Value(0) }];
    let mut c = vec![0xe8, 0, 0, 0, 0];
    link(&mut c, base, &r, &|_| 0x1000 + 5 + 0x100).unwrap();
    assert_eq!(&c[1..5], &0x100i32.to_le_bytes());
    // 3 GB away: no veneer -> error; a veneer at 0x2000 -> the field reaches it
    let far = base + (3u64 << 30);
    let mut c = vec![0xe8, 0, 0, 0, 0];
    assert!(link(&mut c, base, &r, &|_| far).is_err());
    let asked = std::cell::Cell::new(None);
    link_with(&mut c, base, &r, &|_| far, &|k, t, pc| { asked.set(Some((k, t, pc))); Some(0x2000) }).unwrap();
    assert_eq!(asked.get(), Some((RelocKind::X64Branch32, far, base + 5)));
    assert_eq!(&c[1..5], &((0x2000 - (base + 5)) as i32).to_le_bytes());
}

#[test]
fn a64_bl_in_reach_and_through_veneer() {
    let base = 0x10_0000u64;
    let r = [RelocAt { at: 4, kind: RelocKind::A64Branch26, target: Reloc::Value(0) }];
    let mut c = vec![0u8; 8];
    c[4..8].copy_from_slice(&0x9400_0000u32.to_le_bytes());
    // backwards in reach: -0x1000 bytes
    link(&mut c, base, &r, &|_| base + 4 - 0x1000).unwrap();
    assert_eq!(u32::from_le_bytes([c[4], c[5], c[6], c[7]]), 0x9400_0000 | (((-0x1000i32) >> 2) as u32 & 0x03FF_FFFF));
    // 200 MB ahead: out of the 128 MB reach
    let far = base + (200 << 20);
    assert!(!branch_reaches(RelocKind::A64Branch26, base + 4, far));
    assert!(link(&mut c, base, &r, &|_| far).is_err());
    link_with(&mut c, base, &r, &|_| far, &|_, _, pc| Some(pc + 64)).unwrap();
    assert_eq!(u32::from_le_bytes([c[4], c[5], c[6], c[7]]), 0x9400_0000 | 16);
    // a veneer that is itself out of reach is refused
    assert!(link_with(&mut c, base, &r, &|_| far, &|_, t, _| Some(t)).is_err());
}

#[test]
fn thunk_heads() {
    // x64: jmp rel32 at the thunk, else the table-word path at +8
    let at = 0x7000_0000u64;
    let h = relax_backend::x64::thunk_head(at, at + 0x40).to_le_bytes();
    assert_eq!(h[0], 0xe9);
    assert_eq!(i32::from_le_bytes([h[1], h[2], h[3], h[4]]), 0x40 - 5);
    let t = relax_backend::x64::thunk(0x1234_5678_9abc);
    assert_eq!(&relax_backend::x64::thunk_head(at, at + (3u64 << 30)).to_le_bytes(), &t[0..8]);
    assert_eq!(&relax_backend::x64::thunk_head(at, 0).to_le_bytes(), &t[0..8]);
    assert_eq!(t.len(), relax_backend::x64::THUNK_SIZE);
    // arm64: b imm26, else b +4
    let a = relax_backend::arm64::thunk_head(at, at - 8);
    assert_eq!(a, 0x1400_0000 | (((-8i32) >> 2) as u32 & 0x03FF_FFFF));
    let t = relax_backend::arm64::thunk(0x1234_5678_9abc);
    assert_eq!(&relax_backend::arm64::thunk_head(at, at + (200 << 20)).to_le_bytes(), &t[0..4]);
    assert_eq!(u64::from_le_bytes(t[16..24].try_into().unwrap()), 0x1234_5678_9abc);
    assert_eq!(t.len(), relax_backend::arm64::THUNK_SIZE);
}

#[test]
fn count_stubs_jump_directly_in_reach() {
    let at = 0x7000_0000u64;
    // x64: mov rax, counter (10) ; sub [rax], 1 (4) ; je (2) ; jmp rel32 code
    let s = relax_backend::x64::count_stub(at, 0x1234_5678_9abc, 7, at + 0x1000, 0x5555_0000_0000);
    assert_eq!(s[16], 0xe9);
    assert_eq!(i32::from_le_bytes([s[17], s[18], s[19], s[20]]) as i64, 0x1000 - 21);
    assert_eq!(s[15] as usize, 5, "je skips the direct jmp");
    assert!(s.len() <= relax_backend::x64::COUNT_STUB_SIZE);
    // unknown address: the absolute form
    let f = relax_backend::x64::count_stub(0, 0x1234_5678_9abc, 7, at + 0x1000, 0x5555_0000_0000);
    assert_ne!(f[16], 0xe9);
    // arm64: 4 words counter, ldr/subs/str, b.eq, then b code
    let s = relax_backend::arm64::count_stub(at, 0x1234_5678_9abc, 7, at + 0x1000, 0x5555_0000_0000);
    let w = |k: usize| u32::from_le_bytes(s[4 * k..4 * k + 4].try_into().unwrap());
    assert_eq!(w(7), 0x5400_0000 | 2 << 5, "b.eq skips the direct b");
    assert_eq!(w(8), 0x1400_0000 | ((0x1000 - 32) >> 2));
    assert!(s.len() <= relax_backend::arm64::COUNT_STUB_SIZE);
}
