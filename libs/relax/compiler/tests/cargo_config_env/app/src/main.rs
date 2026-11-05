// `include_str!(concat!(env!(..), ..))` through a relative [env] path, and cargo's precedence.
const GREETING: &str = include_str!(concat!(env!("CCE_SOURCES"), "/greeting.txt"));

fn main() {
    println!("config env {} | {} | {} | {} | {}", GREETING.trim(), env!("CCE_FORCED"), env!("CCE_PLAIN"), env!("CCE_SCRIPT"), env!("CCE_OVERRIDE"));
}
