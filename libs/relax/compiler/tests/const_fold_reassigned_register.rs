// check-pass: `x` is assigned again in the loop, so `if x != 0` is not a constant branch
// (a fold that searched for the register's last definition would take it as false and the
// loop would never end: the check would report a long-running constant evaluation).
const C: u32 = {
    let mut x = 0u32;
    let mut n = 0u32;
    loop {
        if x != 0 {
            n = 5;
            break;
        }
        x = 1;
    }
    n
};

fn main() {
    assert_eq!(C, 5);
}
