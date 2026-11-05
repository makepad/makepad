mod mqtt;
#[test]
fn run() { println!("out {}", mqtt::next() >= 7); }
