pub fn add(a: u32, b: u32) -> u32 {
    a + b
}

#[cfg(test)]
mod tests {
    #[test]
    fn lib_add() {
        assert_eq!(super::add(2, 3), 5);
    }
}
