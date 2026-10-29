fn main() {
    let (qualified, imported) = macro_expanded_export_app::picks();
    println!("{} {}", qualified, imported);
}
