#[cfg(test)]
mod tests {
    #[test]
    fn made_is_four() {
        assert_eq!(dc_dep::made().a, 4);
    }

    #[test]
    fn plain() {
        assert_eq!(1 + 1, 2);
    }
}
