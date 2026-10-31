fn main() { dep_a::touch(); println!("{} {} {} {}", top::sub::qualified(), top::unqualified(), top::in_body(), top::via_dollar_crate()); }
