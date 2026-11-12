// relax issue 62: `[S]::join` with an `&OsStr` separator (std's
// `impl<S: Borrow<OsStr>> Join<&OsStr> for [S]`) was missing, so `join` on a `Vec<&OsStr>` was
// E0599; a build script that joins a path's components with "/" did not compile.
use std::ffi::{OsStr, OsString};
use std::path::Path;

#[test]
fn os_str_slices_join_with_an_os_str_separator() {
    let parts: Vec<&OsStr> = vec![OsStr::new("usr"), OsStr::new("lib"), OsStr::new("relax")];
    assert_eq!(parts.join(OsStr::new("/")), OsString::from("usr/lib/relax"));
    let owned = [OsString::from("left"), OsString::from("right")];
    assert_eq!(owned.join(OsStr::new(", ")), OsString::from("left, right"));
    let none: [&OsStr; 0] = [];
    assert_eq!(none.join(OsStr::new("-")), OsString::new());
    assert_eq!([OsStr::new("only")].join(OsStr::new("-")), OsString::from("only"));
    let path = Path::new("src/arch/x86/add.S");
    let components = path.components().map(|c| c.as_os_str()).collect::<Vec<_>>();
    assert_eq!(components.join(OsStr::new("/")), OsString::from("src/arch/x86/add.S"));
}

#[test]
fn allocs_joins_resolve_beside_the_os_str_one() {
    assert_eq!(["x", "y"].join("-"), "x-y");
    assert_eq!([vec![1, 2], vec![3]].join(&0), vec![1, 2, 0, 3]);
    assert_eq!([vec![1], vec![2]].join(&[7, 8][..]), vec![1, 7, 8, 2]);
}
