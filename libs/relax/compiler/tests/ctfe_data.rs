// Constant initializers evaluated to data (libs/relax/compiler/src/ctfe.rs): the shapes of lookup
// tables -- struct rows built by a const fn, (char, char, enum) ranges, niche-encoded Options,
// strings, nested references, repeats, arithmetic and casts. rustc prints the same hash:
// 3413466678543477432. MAKEPAD_RUN_CTFE=check also compiles and runs every initializer and
// compares the values.

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tag(pub u32);
impl Tag {
    pub const fn from_bytes(bytes: &[u8; 4]) -> Self {
        Tag(((bytes[0] as u32) << 24) | ((bytes[1] as u32) << 16) | ((bytes[2] as u32) << 8) | (bytes[3] as u32))
    }
}

#[derive(Clone, Copy)]
pub struct LangTag {
    pub language: &'static str,
    pub tag: Tag,
}

pub const LANGS: &[LangTag] = &[
    LangTag { language: "aa", tag: Tag::from_bytes(b"AFR ") },
    LangTag { language: "aae", tag: Tag::from_bytes(b"SQI ") },
    LangTag { language: "aba", tag: Tag(0) },
    LangTag { language: "zh-Hant", tag: Tag::from_bytes(b"ZHT ") },
];

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Cat {
    Cc,
    Lu,
    Ll,
    Nd,
    Zs,
}

pub const RANGES: &[(char, char, Cat)] = &[('\u{0}', '\u{1f}', Cat::Cc), ('0', '9', Cat::Nd), ('A', 'Z', Cat::Lu), ('a', 'z', Cat::Ll), ('\u{3000}', '\u{3000}', Cat::Zs)];

#[derive(Clone, Copy)]
pub enum Shape {
    Unit,
    Pair(i16, u8),
    Named { w: u32, h: u32 },
}

pub static SHAPES: [Shape; 3] = [Shape::Unit, Shape::Pair(-7, 200), Shape::Named { w: 3, h: 4 }];
pub const OPTS: [Option<&str>; 3] = [None, Some("x"), Some("")];
pub const OPT_U8: [Option<u8>; 2] = [Some(5), None];
pub const NESTED: &[&[u16]] = &[&[1, 2, 3], &[], &[0xffff]];
pub const REPEAT: [u32; 6] = [0x1234_5678; 6];
pub const ARITH: (i64, u8, i32, u64, bool, char) = (-(3 * 7 - 100) << 2, (300u32 % 256) as u8, -5 / 2, !0u64 >> 4, 3 > 2 && 1 != 1 || true, 65u8 as char);
pub const FLOATS: [f64; 3] = [1.5, -0.25, 1e10];
pub const F32S: (f32, f32) = (0.1, -2.0);
pub const BASE: u32 = 40;
pub const DERIVED: u32 = BASE * 2 + 1;
pub const ARR_REF: &[i8; 4] = &[-1, 0, 1, 127];
pub const SLICE: &[i8] = ARR_REF;
pub const TUPLE_IDX: u8 = (1u8, 2u8).1 + [10u8, 20][1];

fn mix(h: &mut u64, x: u64) {
    *h = (*h ^ x).wrapping_mul(0x100_0000_01b3);
}

#[test]
fn ctfe_data() {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for l in LANGS {
        mix(&mut h, l.tag.0 as u64);
        for b in l.language.bytes() {
            mix(&mut h, b as u64);
        }
    }
    for (a, b, c) in RANGES {
        mix(&mut h, *a as u64);
        mix(&mut h, *b as u64);
        mix(&mut h, *c as u64);
    }
    for s in SHAPES.iter() {
        match s {
            Shape::Unit => mix(&mut h, 1),
            Shape::Pair(a, b) => {
                mix(&mut h, *a as u64);
                mix(&mut h, *b as u64)
            }
            Shape::Named { w, h: hh } => {
                mix(&mut h, *w as u64);
                mix(&mut h, *hh as u64)
            }
        }
    }
    for o in OPTS {
        mix(&mut h, o.map_or(99, |s| s.len() as u64));
    }
    for o in OPT_U8 {
        mix(&mut h, o.map_or(77, |x| x as u64));
    }
    for s in NESTED {
        mix(&mut h, s.len() as u64);
        for x in s.iter() {
            mix(&mut h, *x as u64);
        }
    }
    for x in REPEAT {
        mix(&mut h, x as u64);
    }
    mix(&mut h, ARITH.0 as u64);
    mix(&mut h, ARITH.1 as u64);
    mix(&mut h, ARITH.2 as u64);
    mix(&mut h, ARITH.3);
    mix(&mut h, ARITH.4 as u64);
    mix(&mut h, ARITH.5 as u64);
    for f in FLOATS {
        mix(&mut h, f.to_bits());
    }
    mix(&mut h, F32S.0.to_bits() as u64);
    mix(&mut h, F32S.1.to_bits() as u64);
    mix(&mut h, DERIVED as u64);
    for x in SLICE {
        mix(&mut h, *x as u64);
    }
    mix(&mut h, TUPLE_IDX as u64);
    println!("hash {}", h);
    assert_eq!(h, 3413466678543477432);
}
