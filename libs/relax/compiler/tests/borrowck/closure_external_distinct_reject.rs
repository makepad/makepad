// External closure requirements must still be checked in the parent.
struct Data<'a>(&'a u32);
fn bad<'a, 'b>(v: Option<Data<'a>>) -> Option<&'b u32> {
    v.map(|d| d.0)
}
fn main() {}
