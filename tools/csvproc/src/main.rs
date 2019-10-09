use std::fs;

fn main(){
    let st = fs::read_to_string("~/Downloads/data.csv").unwrap();
    println!("{}", st);
}
