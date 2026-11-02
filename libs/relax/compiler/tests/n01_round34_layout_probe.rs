// Layouts of 4 GiB and more: rustc accepts them on a 64-bit target.
pub fn array_size() -> usize { std::mem::size_of::<[u8; 1usize << 32]>() }
pub fn nested_size() -> usize { std::mem::size_of::<([u8; 1usize << 31], [u8; 1usize << 31])>() }

#[test]
fn large_layout_regression() {
    assert_eq!(array_size(), 1usize << 32);
    assert_eq!(nested_size(), 1usize << 32);
}
