// rustybuzz face.rs: a borrowed field with an independent inner lifetime.
struct Data<'a> { bytes: &'a [u8] }
struct Table<'a> { inner: Data<'a> }
fn field<'a, 'b>(v: &'b Option<Table<'a>>) -> Option<&'b Data<'a>> {
    v.as_ref().map(|t| &t.inner)
}
fn main() {}
