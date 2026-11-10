// E0436, struct update syntax on an enum variant: a check only the validating front end makes.
// rustc refuses this program, so `makepad run` must too, before main.
#[derive(Debug)]
enum Edit {
    Set { name: String, size: u32 },
}

fn set(name: &str) -> Edit {
    Edit::Set { name: name.to_string(), size: 3 }
}

fn main() {
    println!("started");
    println!("{:?}", Edit::Set { size: 4, ..set("x") });
}
