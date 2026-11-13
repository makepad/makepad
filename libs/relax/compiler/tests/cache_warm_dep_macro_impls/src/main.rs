use codec::Encode;

fn main() {
    println!("cache_warm_dep_macro_impls {:?} {:?}", [5u8; 3].encode(), [6u8; 1].encode());
}

#[test]
fn each_length_has_its_impl() {
    assert_eq!([5u8; 3].encode(), [5, 5, 5, 3]);
    assert_eq!([6u8; 1].encode(), [6, 1]);
}
