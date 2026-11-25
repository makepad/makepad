// rejects: lifetime may not live long enough
// The projection's arguments imply `'v: 'a` for `Cursor`, never `'a: 'v`.
pub struct Entry<'v> {
    pub key: &'v str,
}
pub struct Cursor<'a, 'v> {
    pub inner: <&'a [Entry<'v>] as IntoIterator>::IntoIter,
}
pub fn widen<'a, 'v>(_c: &Cursor<'a, 'v>, s: &'a str) -> &'v str {
    s
}
