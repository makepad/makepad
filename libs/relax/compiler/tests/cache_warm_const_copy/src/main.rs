// Constants whose values refer to a static are copied into a per-domain block
// (domain.rs rehome_const); one whose integers only look like addresses is not. A warm run
// links the code reading them from the disk cache and must print the cold run's line: the
// cache once named the per-domain copy as a fresh, zeroed block, so a warm run read a null
// pointer and a length of 0 (the warm Stage hang: unicode-segmentation's word_cat_table).
static S: u64 = 7;
static WORDS: [u64; 3] = [3, 5, 8];

const C: &u64 = &S;

struct Table {
    at: &'static u64,
    items: &'static [u64],
    len: usize,
}

const T: Table = Table { at: &S, items: &WORDS, len: 1085 };

// integers only: never copied, whatever their values are in this run
const PAIRS: &[(u32, u32)] = &[(1, 2), (3, 4), (5, 6), (7, 8), (0x41, 0x5a), (0x61, 0x7a)];

#[inline(never)]
fn read() -> u64 {
    let pairs: u64 = PAIRS.iter().map(|&(a, b)| (a + b) as u64).sum();
    *C * 1_000_000 + *T.at * 100_000 + T.items.iter().sum::<u64>() * 1000 + T.len as u64 + pairs + PAIRS.len() as u64
}

fn main() {
    println!("cache_warm_const_copy result {}", read());
}
