//! A reference to a struct of closures that capture nothing is promoted to a 'static: rustc
//! promotes such a closure as a constant aggregate, and its reification to a `fn` pointer too.
//! `&Table { len: |s| s.len(), .. }` returned from a function (a type-erasing vtable's shape)
//! outlives it; a stack temporary would be overwritten by the next call.
struct Pair {
    first: fn() -> usize,
    second: fn() -> usize,
}

struct Table {
    len: fn(&str) -> usize,
    twice: fn(usize) -> usize,
    name: fn() -> &'static str,
}

fn pair() -> &'static Pair {
    &Pair { first: || 10, second: || 20 }
}

fn table() -> &'static Table {
    &Table { len: |s| s.len(), twice: |n| n * 2, name: || "t" }
}

// where `table()` puts its value, called `depth` frames further down the stack (each call an
// indirect one, and the recursion no tail call: no frame is inlined or reused)
fn table_at(depth: u32) -> *const Table {
    if std::hint::black_box(depth) == 0 {
        std::hint::black_box(table as fn() -> &'static Table)()
    } else {
        std::hint::black_box(table_at(depth - 1))
    }
}

fn clobber() -> u64 {
    let a = std::hint::black_box([0xdead_beef_u64; 64]);
    a.iter().fold(0, |x, y| x ^ y)
}

#[test]
fn promoted_closures_outlive_their_function() {
    assert_eq!(table_at(0), table_at(16), "one static, not a temporary in each caller's frame");
    // (through `fn` pointers: an inlined call would put the value in this frame)
    let (get_pair, get_table) = std::hint::black_box((pair as fn() -> &'static Pair, table as fn() -> &'static Table));
    let (p, t) = (get_pair(), get_table());
    std::hint::black_box(clobber());
    assert_eq!(((p.first)(), (p.second)()), (10, 20), "two closures after the stack was reused");
    assert_eq!(((t.len)("abc"), (t.twice)(21), (t.name)()), (3, 42, "t"), "closures that take arguments");
}
