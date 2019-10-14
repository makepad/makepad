use std::fs;

fn main() {
    let data = fs::read_to_string("data.csv").unwrap();
    let lines: Vec<&str> = data.split("\n").collect();

    for (index, line) in lines.iter().enumerate() {
        let chunks: Vec<&str> = line.split("\",\"").collect();
        if index == 0 {
            //for chunk in &chunks{
            //println!("{}", chunk);
            //}
        }
        else if chunks[0].len()>0 {
            let _date = &chunks[0][1..]; 
            let name = &chunks[1];
            //println!("{}", name);
            let addsub = if chunks[5].len() == 2 {"-"}else {""};
            let value = &chunks[6];
            let _misc = &chunks[8];
            let mut lcname = name.to_string();
            lcname.make_ascii_lowercase(); 
            if lcname.find("albert").is_some(){
                //println!("{} {}{} {} {}", date, addsub, value, name, _misc);
                println!("{}{}", addsub, value);
            }
            else {
            }
        }
    }
}
