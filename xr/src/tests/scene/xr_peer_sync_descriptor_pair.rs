mod descriptor_pair_tests {
    use super::descriptor_pair_ready_for_solve;

    #[test]
    fn initial_descriptor_pair_solves_immediately() {
        assert!(descriptor_pair_ready_for_solve(None, None, (1, 0), 7));
    }

    #[test]
    fn one_sided_descriptor_updates_resolve_immediately() {
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (2, 0),
            7,
        ));
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (1, 0),
            8,
        ));
        assert!(descriptor_pair_ready_for_solve(
            Some((1, 0)),
            Some(7),
            (2, 0),
            8,
        ));
    }
}
