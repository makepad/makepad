//! `PathBuf` and `OsString` parse from a str, as std's `FromStr` impls (their `Err` is
//! `Infallible`): `PathBuf::from_str` with the trait imported, and `str::parse` (relax issue 35).
use std::convert::Infallible;
use std::ffi::OsString;
use std::path::PathBuf;
use std::str::FromStr;

#[test]
fn paths_and_os_strings_parse_from_str() {
    let path = PathBuf::from_str("dir/test.xml").unwrap();
    assert_eq!(path, PathBuf::from("dir/test.xml"), "PathBuf::from_str");
    assert_eq!(path.file_name().and_then(|n| n.to_str()), Some("test.xml"), "a parsed path's parts");
    let parsed: Result<PathBuf, Infallible> = "a/b".parse();
    assert_eq!(parsed, Ok(PathBuf::from("a/b")), "str::parse into a PathBuf, Err = Infallible");
    let os: Result<OsString, Infallible> = OsString::from_str("x y");
    assert_eq!(os, Ok(OsString::from("x y")), "OsString::from_str");
}
