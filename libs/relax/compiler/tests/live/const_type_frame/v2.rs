// A const a type uses (an array length) computed by a const fn from a const whose initializer
// builds its array in a frame slot (fields of other consts). Initializer code's frame slots are
// not fn slots: a body edit elsewhere patches, every time (the walk of the initializer's code
// once took frame slot 0 for fn slot 0 and reached whatever fn that was).
struct Face {
    path: &'static str,
}

const REGULAR: Face = Face { path: "regular.ttf" };
const BOLD: Face = Face { path: "bold.ttf" };
const LAZY: &[&str] = &[REGULAR.path, BOLD.path];

const fn total(parts: &[&str]) -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < parts.len() {
        n += parts[i].len();
        i += 1;
    }
    n
}

const LEN: usize = total(LAZY);
static TABLE: [u8; LEN] = [7; LEN];

fn kernel(x: u64) -> u64 {
    x + 3
}

#[test]
fn run() {
    println!("out len {} k {}", TABLE.len(), kernel(1));
}
