fn greet(args: &[String]) -> String {
    format!("hello from bin {} {}", args.join(","), bt_app::add(20, 22))
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    println!("{}", greet(&args));
}

#[cfg(test)]
mod tests {
    #[test]
    fn bin_greet() {
        assert_eq!(super::greet(&["a".to_string()]), "hello from bin a 42");
    }
}
