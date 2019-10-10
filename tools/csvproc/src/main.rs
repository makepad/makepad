use std::fs;

fn main() {
    let data = fs::read_to_string("data.csv").unwrap();
    
    let lines: Vec<&str> = data.split("\n").collect();
    //for i in 0..1{
    for (index, line) in lines.iter().enumerate() {
        let chunks: Vec<&str> = line.split("\",\"").collect();
        if index == 0 {
            //for chunk in &chunks{
            //println!("{}", chunk);
            //}  
        } 
        else if chunks[0].len()>0 {
            let date = &chunks[0][1..];
            let name = &chunks[1];
            //println!("{}", name);
            let addsub = if chunks[5].len() == 2 {"-"}else {""};
            let value = &chunks[6];
            let misc = &chunks[8];
            let mut lcname = name.to_string(); 
            lcname.make_ascii_lowercase();        
            if let Some(_) = lcname.find(""){  
                println!("{} {}{} {}", date, addsub, value, name); 
            }
             
            //println!("{}", misc);
            //println!("{}",date);
            //   println!("-{}", value); 
            //}
            // else{
            //    println!("{}", value);
            //println!("{} {} {}", date, value, name);
            // }
        }
    }
}
