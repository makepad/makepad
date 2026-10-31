// std-core differential test (core_diff.sh): str, String, char.

#[test]
fn a01_str_basics() {
    let s = "  Hello, Wörld! héllo  ";
    let t = s.trim();
    println!("| trim [{}] {} {}", t, t.len(), t.chars().count());
    println!("| find {} rfind {}", t.find("llo").unwrap(), t.rfind('l').unwrap());
    println!("| starts {} ends {}", t.starts_with("Hell"), t.ends_with("lo"));
    println!("| upper {}", t.to_uppercase());
    println!("| lower {}", t.to_lowercase());
    let mut n = 0;
    for part in t.split(", ") {
        n += 1;
        println!("| part {}", part);
    }
    println!("| parts {}", n);
    for w in t.split_whitespace() {
        println!("| word {}", w);
    }
    println!("| replace {}", t.replace("l", "L"));
    println!("| strip {}", t.strip_prefix("Hello").unwrap());
    println!("| sub {}", &t[7..13]);
}

#[test]
fn a02_string_build() {
    let mut s = String::new();
    s.push_str("abc");
    s.push('é');
    s.push('z');
    println!("| {} {}", s, s.len());
    s.insert(1, 'X');
    s.insert_str(0, ">>");
    println!("| {}", s);
    println!("| pop {}", s.pop().unwrap());
    println!("| remove {}", s.remove(0));
    s.truncate(4);
    println!("| {}", s);
    let t = s.clone() + "-tail";
    println!("| {}", t);
    let rev: String = t.chars().rev().collect();
    println!("| {}", rev);
    let up: String = t.chars().map(|c| c.to_ascii_uppercase()).collect();
    println!("| {}", up);
}

#[test]
fn a03_parse() {
    println!("| {}", "123".parse::<u32>().unwrap());
    println!("| {}", "-45".parse::<i64>().unwrap());
    println!("| {}", "300".parse::<u8>().is_err());
    println!("| {}", "2.5".parse::<f64>().unwrap());
    println!("| {}", "1e3".parse::<f32>().unwrap());
    println!("| {}", "true".parse::<bool>().unwrap());
    println!("| {}", u32::from_str_radix("ff", 16).unwrap());
}

#[test]
fn a04_chars() {
    let s = "aZ9 é€😀";
    for c in s.chars() {
        println!(
            "| {} {} {} {} {} {} {}",
            c as u32,
            c.is_alphabetic(),
            c.is_numeric(),
            c.is_whitespace(),
            c.is_uppercase(),
            c.len_utf8(),
            c.to_digit(36).unwrap_or(99)
        );
    }
    for (i, c) in s.char_indices() {
        print!("| {}:{}", i, c);
    }
    println!();
    let b = s.as_bytes();
    println!("| bytes {} {} {}", b.len(), b[0], b[b.len() - 1]);
    println!("| utf8 {}", std::str::from_utf8(b).unwrap());
    println!("| bad {}", std::str::from_utf8(&b[..6]).is_err());
}

#[test]
fn a05_lines() {
    let text = "one\ntwo\r\nthree\n\nfive";
    let mut n = 0;
    for l in text.lines() {
        n += 1;
        println!("| line [{}]", l);
    }
    println!("| count {}", n);
    let mut it = text.splitn(3, '\n');
    println!("| {} {}", it.next().unwrap(), it.next().unwrap());
    let (a, b) = "key=value=x".split_once('=').unwrap();
    println!("| {} {}", a, b);
}
