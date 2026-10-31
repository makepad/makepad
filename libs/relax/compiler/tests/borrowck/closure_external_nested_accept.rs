// rustybuzz face.rs: nested closures return a captured value and borrowed ADT.
struct Data<'a> { bytes: &'a [u8] }
struct Table<'a> { inner: Data<'a> }
fn nested<'a>(v: &'a Option<Table<'a>>) -> impl Iterator<Item=(u8, &'a Data<'a>)> {
    [0u8].into_iter().filter_map(move |i| v.as_ref().map(|t| (i, &t.inner)))
}
fn main() {}
