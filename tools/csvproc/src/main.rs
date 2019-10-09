use std::fs;

fn main(){
    println!("Hi");   
    let st = fs::read_to_string("/Users/Admin/Downloads/data.csv").unwrap();
    println!("{}", st);
}
