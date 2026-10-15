#[path = "const_param_env/src/lib.rs"]
mod reduced;

#[test]
fn nested_const_closures_deduplicate_and_flush() {
    let sort = reduced::ExternalSort(vec![[1, 0], [1, 1], [2, 0], [3, 0]]);
    assert_eq!(reduced::indexed(sort, 1).unwrap(), vec![vec![1, 0], vec![2, 0], vec![3, 0]]);
    assert_eq!(reduced::indexed(reduced::ExternalSort::<2>(Vec::new()), 1).unwrap(), Vec::<Vec<u8>>::new());
}

#[test]
fn impl_const_environment_reaches_scoped_worker() {
    let mut sort = reduced::ExternalSort::<3>(Vec::new());
    std::thread::scope(|scope| {
        let feed = reduced::Feed::start(scope, &mut sort);
        feed.send(vec![[1, 2, 3], [4, 5, 6]]);
        feed.join();
    });
    assert_eq!(sort.0, vec![[1, 2, 3], [4, 5, 6]]);
    let rows = reduced::Rows(sort.0);
    assert_eq!(rows.bytes(), rows.0.capacity() * 3);
}

#[test]
fn substituted_callees_keep_caller_const_types() {
    assert_eq!(reduced::forwarded::<true, 3>([7, 8, 9]), ([7, 8, 9], [7, 8, 9]));
    assert_eq!(reduced::forwarded::<false, 0>([]), ([], []));
}

#[test]
fn substituted_comparisons_resolve_with_const_clauses() {
    assert!(reduced::ordered([1, 2], [1, 2]));
    assert!(reduced::ordered([1, 2], [1, 3]));
    assert!(!reduced::ordered([1, 3], [1, 2]));
}
