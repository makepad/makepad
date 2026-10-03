// HotRust: #[cfg] on statements inside fn bodies is stripped before type checking.

fn pick() -> i64 {
    let mut x = 1;
    #[cfg(target_os = "nonexistent_os")]
    {
        x += 100;
    }
    #[cfg(not(target_os = "nonexistent_os"))]
    {
        x += 10;
    }
    #[cfg(target_os = "nonexistent_os")]
    let y = 1000;
    #[cfg(not(target_os = "nonexistent_os"))]
    let y = 0;
    #[cfg(target_os = "nonexistent_os")]
    {
        x += missing_function();
    }
    x + y
}

fn tail() -> i64 {
    #[cfg(target_os = "nonexistent_os")]
    {
        1
    }
    #[cfg(not(target_os = "nonexistent_os"))]
    {
        2
    }
}

#[test]
fn cfg_statements() {
    assert_eq!(pick(), 11);
    assert_eq!(tail(), 2);
}
