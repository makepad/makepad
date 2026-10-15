// T5 layouts above 4 GiB; owners: types + codegen. Known failing fixture, deferred by ROUND 35.
// S5 boundary: rustc accepts a 4 GiB layout on a 64-bit target.
pub fn array_size() -> usize { std::mem::size_of::<[u8; 1usize << 32]>() }
pub fn nested_size() -> usize { std::mem::size_of::<([u8; 1usize << 31], [u8; 1usize << 31])>() }

#[test]
#[ignore = "T5: layouts above 4 GiB; types and codegen"]
fn large_layout_regression() {
    assert_eq!(array_size(), 1usize << 32);
    assert_eq!(nested_size(), 1usize << 32);
}
