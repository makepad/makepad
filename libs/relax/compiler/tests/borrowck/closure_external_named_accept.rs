// ttf-parser aat.rs / morx.rs: map an ADT into Self with a named lifetime.
struct Data<'a> { bytes: &'a [u8] }
struct Table<'a> { data: Data<'a> }
impl<'a> Table<'a> {
    fn parse(data: Option<Data<'a>>) -> Option<Self> {
        data.map(|data| Self { data })
    }
}
fn main() {}
