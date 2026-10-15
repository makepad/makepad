// std-os lane: std::path (differential: os_diff.sh)
use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

const CASES: [&str; 26] = [
    "", "/", "//", "a", "/a", "a/", "a/b", "/a/b/", "./a", ".", "..", "./", "a/./b", "a/../b", "a//b",
    "/a/b/c.txt", "c.tar.gz", ".hidden", "a/.hidden", "foo.", "..x", "/..", "./..", "a/b/..", "///x//y///", "x/./",
];

#[test]
fn a01_components() {
    for c in CASES.iter() {
        let p = Path::new(c);
        let comps: Vec<Component> = p.components().collect();
        let back: Vec<Component> = p.components().rev().collect();
        println!("| {:?} -> {:?} rev {:?}", c, comps, back);
        let mut it = p.components();
        it.next();
        println!("|   as_path after 1: {:?} {:?}", it.as_path(), p.components());
        let mut it = p.components();
        it.next_back();
        println!("|   as_path after back: {:?}", it.as_path());
    }
}

#[test]
fn a02_queries() {
    for c in CASES.iter() {
        let p = Path::new(c);
        println!("| {:?} parent {:?} name {:?} stem {:?} ext {:?} abs {} root {}", c, p.parent(), p.file_name(), p.file_stem(), p.extension(), p.is_absolute(), p.has_root());
        let anc: Vec<&Path> = p.ancestors().collect();
        let iter: Vec<&OsStr> = p.iter().collect();
        println!("|   anc {:?} iter {:?} disp {}", anc, iter, p.display());
    }
}

#[test]
fn a03_build() {
    let mut p = PathBuf::from("/usr");
    p.push("lib");
    p.push("x.so");
    let popped = p.pop();
    println!("| {:?} {}", p, popped);
    p.push("/etc/passwd");
    println!("| {:?}", p);
    let mut q = PathBuf::new();
    q.push("rel");
    q.push("");
    q.push("x");
    println!("| {:?} {:?}", q, Path::new("a/").join("b"));
    let mut r = PathBuf::from("dir/file.txt");
    println!("| {} {:?}", r.set_extension("md"), r);
    println!("| {} {:?}", r.set_extension(""), r);
    r.set_file_name("other.rs");
    println!("| {:?} {:?} {:?}", r, r.with_extension("o"), r.with_file_name("z"));
    let mut e = PathBuf::from("/");
    let a = e.set_extension("x");
    let b = e.pop();
    println!("| {} {} {:?}", a, b, e);
    println!("| {:?}", Path::new("a/b/c").strip_prefix("a/b"));
    println!("| {:?}", Path::new("a/b/c").strip_prefix("a/x").map_err(|e| e.to_string()));
    println!("| {:?}", Path::new("/a/b").strip_prefix("/"));
    println!("| {} {} {} {}", Path::new("/a/b/c").starts_with("/a"), Path::new("/a/b/c").starts_with("/a/b/c/"), Path::new("a/bc").starts_with("a/b"), Path::new("a/b/c").ends_with("b/c"));
    println!("| {} {}", Path::new("a/b") == Path::new("a//b/"), Path::new("./a") == Path::new("a"));
    println!("| {:?} {:?}", Path::new("a").cmp(Path::new("b")), PathBuf::from("a/b") < PathBuf::from("a/b/c"));
    let joined: PathBuf = ["x", "y", "z.txt"].iter().collect();
    println!("| {:?} {:?}", joined, joined.to_str());
    let os = OsString::from("ab");
    let mut os2 = os.clone();
    os2.push("cd");
    println!("| {:?} {:?} {:?} {}", os, os2, os2.to_str(), os2.len());
    println!("| {:?} {:?}", PathBuf::from(os2).into_os_string(), Path::new("a\u{e9}\"b\n").to_string_lossy());
    println!("| {:?} {}", Path::new("q\u{e9}\t\"'"), Path::new("q\u{e9}").display());
}
