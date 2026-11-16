mod first;
mod second;

fn main() {
    println!("{} {}", first::bump(), second::bump());
}
