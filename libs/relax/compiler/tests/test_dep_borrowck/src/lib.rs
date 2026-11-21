#[cfg(test)]
mod t {
    #[test]
    fn uses_dep() {
        assert_eq!(tdb_dep::add(1, 2), 3);
        assert_eq!(tdb_private::store::Store::new().len(), 7);
    }
}
