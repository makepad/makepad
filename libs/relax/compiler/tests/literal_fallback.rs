// Integer literal variables stay open until the whole body is checked (rustc_hir_typeck
// fallback.rs: `{integer}` -> i32 only after typeck of the body), boot lane B1c
// (objc-sys encode.rs `let mut bytes = [0; CAP]` filled from `code.bytes()`). rustc: all pass.
const CAP: usize = 8;

enum Code {
    Inline(u8, [u8; CAP]),
    #[allow(dead_code)]
    Other,
}

fn inline(code: &str) -> Code {
    let mut bytes = [0; CAP];
    for (dst, byte) in bytes.iter_mut().zip(code.bytes()) {
        *dst = byte;
    }
    Code::Inline(code.len() as u8, bytes)
}

fn first_even(v: &[u32]) -> Option<&u32> {
    v.iter().find(|x| **x % 2 == 0)
}

#[test]
fn literals_learn_from_later_uses() {
    match inline("ab") {
        Code::Inline(n, b) => assert!(n == 2 && b[1] == b'b' && b[2] == 0),
        Code::Other => panic!(),
    }
    let mut a = [0; 4];
    for d in a.iter_mut() {
        *d = 7u8;
    }
    let x: [u8; 4] = a;
    assert!(x[3] == 7);
    let mut it = [0; 2];
    let mut m = it.iter_mut();
    let d = m.next().unwrap();
    *d = 9u16;
    assert!(it[0] == 9u16);
    let v = vec![3, 5, 8, 9];
    assert!(first_even(&v).copied() == Some(8));
    let y = 1 << 40;
    let z: u64 = y;
    assert!(z == 1u64 << 40);
    let f = 1.5;
    let g: f32 = f;
    assert!(g == 1.5f32);
    let i = 7;
    assert!(i / 2 == 3);
}
