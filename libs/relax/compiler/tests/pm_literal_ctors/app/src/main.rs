// proc_macro's Literal constructors, Literal::c_string among them (relax issue 103): the text each
// makes and the value the program reads from it, as under rustc.
plc_mac::literals!();

fn main() {
    for t in TEXTS {
        println!("{t}");
    }
    println!("{:?}", VALUES);
}
