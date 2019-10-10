use std::fs;

fn main(){
    let data = fs::read_to_string("data.csv").unwrap();

     let thread = std::thread::spawn(move || {
         panic!("Thread panic")
     });
    
    let lines:Vec<&str> = data.split("\n").collect();
    for (index,line) in lines.iter().enumerate(){
        let chunks:Vec<&str> = line.split("\",\"").collect();
        if index == 0{
            //for chunk in &chunks{
                //println!("{}", chunk);
            //}
        }
        else if chunks[0].len()>0{
            let date = &chunks[0][1..];
            let name = &chunks[1];
            let addsub = &chunks[5];
            let value = &chunks[6];
            let misc = &chunks[8];
            if addsub.len() == 2{
                println!("{} -{} {} {}", date, value, name, misc);
            }
            else{
                println!("{} {} {}", date, value, name);
            }
        }
    }
    let _ = thread.join();
}
