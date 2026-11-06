fn main() {
    let mut e = 0;
    let mut m = check_targets::Model(2);
    println!("{}", m.evaluate(&mut e));
}

#[cfg(test)]
mod tests {
    use check_targets::Model;

    // fab's sla.rs: a model made by a boxed closure, evaluated through `&self`
    #[test]
    fn evaluates_in_a_loop() {
        let plates: Vec<(&str, Box<dyn Fn() -> Model>)> = vec![("a", Box::new(|| Model(3)))];
        for (key, model) in &plates {
            let mut e = 0;
            let mut m = model();
            assert_eq!(m.evaluate(&mut e), 4, "{key}");
        }
    }
}
