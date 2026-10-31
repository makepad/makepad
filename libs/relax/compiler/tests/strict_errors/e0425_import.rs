//~ E0425
//~ help: import it: `use crate::util::parse_header;`
mod util { pub fn parse_header() -> u32 { 1 } }
fn main() { let _ = parse_header(); }
