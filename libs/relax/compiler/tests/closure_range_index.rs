#[test]
fn mapped_integer_range_infers_index_type() {
    let values = [3u8; 64];
    let mut sum = 0;
    for i in (0..8).map(|i| 8 * i) {
        sum += values[i + 2];
    }
    assert_eq!(sum, 24);
}
