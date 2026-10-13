#[derive(Clone, Copy, PartialEq, Eq)]
struct Key(u64);
impl Key {
    const SEED: u64 = 0xd6e8_feb8_6659_fd93;
    pub const fn from_bytes(seed: u64, id_bytes: &[u8], start: usize, end: usize, or: u64) -> Key {
        let mut x = seed;
        let mut i = start;
        while i < end {
            x = x.overflowing_add(id_bytes[i] as u64).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            x = x.overflowing_mul(0xd6e8_feb8_6659_fd93).0;
            x ^= x >> 32;
            i += 1;
        }
        Key((x & 0x0000_3fff_ffff_ffff) | or)
    }
}
#[derive(Clone, Copy)]
pub struct Entry {
    pub len: u8,
    pub lanes: [u8; 4],
}

const SLOTS: usize = 2048;

/// The 680 swizzle names by id, open-addressed on the id's low bits (a third
/// full). Built at compile time from the two lane alphabets, so a lookup is a
/// probe or two instead of a 680-arm `match` per call site.
static TABLE: [(Key, Entry); SLOTS] = build_table();

const fn build_table() -> [(Key, Entry); SLOTS] {
    let mut table = [(Key(0), Entry { len: 0, lanes: [0; 4] }); SLOTS];
    let alphabets: [&[u8; 4]; 2] = [b"xyzw", b"rgba"];
    let mut alphabet = 0;
    while alphabet < alphabets.len() {
        let mut len = 1;
        while len <= 4 {
            // Every lane combination of this length, first lane most significant.
            let mut code = 0;
            while code < 1 << (2 * len) {
                let mut name = [0u8; 4];
                let mut lanes = [0u8; 4];
                let mut i = 0;
                while i < len {
                    lanes[i] = ((code >> (2 * (len - 1 - i))) & 3) as u8;
                    name[i] = alphabets[alphabet][lanes[i] as usize];
                    i += 1;
                }
                let id = Key::from_bytes(Key::SEED, &name, 0, len, 0);
                let mut slot = id.0 as usize & (SLOTS - 1);
                while table[slot].0 .0 != 0 {
                    slot = (slot + 1) & (SLOTS - 1);
                }
                table[slot] = (
                    id,
                    Entry {
                        len: len as u8,
                        lanes,
                    },
                );
                code += 1;
            }
            len += 1;
        }
        alphabet += 1;
    }
    table
}

/// The swizzle a field name of a pod vector stands for, if it is one.
pub fn lookup(id: Key) -> Option<Entry> {
    let mut slot = id.0 as usize & (SLOTS - 1);
    loop {
        let (key, swizzle) = TABLE[slot];
        if key.0 == 0 {
            return None;
        }
        if key == id {
            return Some(swizzle);
        }
        slot = (slot + 1) & (SLOTS - 1);
    }
}


#[test]
fn static_const_loop_matches_runtime() {
    let runtime = build_table();
    let mut count = 0;
    for i in 0..SLOTS {
        if runtime[i].0.0 != 0 { count += 1; }
        assert_eq!(TABLE[i].0.0, runtime[i].0.0, "key {}", i);
        assert_eq!(TABLE[i].1.len, runtime[i].1.len, "len {}", i);
        assert_eq!(TABLE[i].1.lanes, runtime[i].1.lanes, "lanes {}", i);
    }
    assert_eq!(count, 680);
    assert_eq!(lookup(Key::from_bytes(Key::SEED, b"xy", 0, 2, 0)).unwrap().len, 2);
}
