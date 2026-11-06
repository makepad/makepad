// A second round of std surface the dependency stack reaches for (chrono/jiff, icu, image,
// constant_time_eq): PhantomData ordering, u8::escape_ascii, str::from_utf8* as associated
// functions, unchecked slice splits, io traits through a Box, OsString/OsStr comparisons and
// the NEON pair load.
use std::io::{BufRead, Cursor, Read, Seek, SeekFrom, Write};
use std::marker::PhantomData;

#[test]
fn a01_phantom_data_orders() {
    let a: PhantomData<&str> = PhantomData;
    let b: PhantomData<&str> = PhantomData;
    println!("| {:?} {:?} {}", a.cmp(&b), a.partial_cmp(&b), a <= b);
}

#[test]
fn a02_escape_ascii_and_str_associated() {
    let s: String = [b'a', b'\n', b'\'', 0x7f, b'"', 0x01].iter().map(|b| b.escape_ascii().to_string()).collect();
    println!("| {s}");
    println!("| {:?} {:?}", str::from_utf8(b"ok"), str::from_utf8(&[0xff]).is_err());
    println!("| {}", unsafe { str::from_utf8_unchecked(b"raw") });
    let mut bytes = *b"mut";
    println!("| {}", unsafe { str::from_utf8_unchecked_mut(&mut bytes) });
}

#[test]
fn a03_unchecked_splits() {
    let v = [1u8, 2, 3, 4, 5];
    let (a, b) = unsafe { v.split_at_unchecked(2) };
    println!("| {:?} {:?}", a, b);
    let mut m = [1u8, 2, 3];
    let (x, y) = unsafe { m.split_at_mut_unchecked(1) };
    x[0] = 9;
    y[1] = 8;
    println!("| {:?}", m);
}

#[test]
fn a04_io_through_a_box() {
    // Sized boxes: the dyn cases (`Box<dyn Read>`) are a separate Relax issue (instance
    // resolution of a default method through a `?Sized` impl), see tests/std/box_dyn_read.rs.
    let mut r: Box<Cursor<Vec<u8>>> = Box::new(Cursor::new(b"hello world".to_vec()));
    let mut s = String::new();
    r.read_to_string(&mut s).unwrap();
    println!("| read {s}");
    let mut w: Box<Vec<u8>> = Box::new(Vec::new());
    w.write_all(b"abc").unwrap();
    w.flush().unwrap();
    println!("| wrote {:?}", *w);
    let mut sk: Box<Cursor<Vec<u8>>> = Box::new(Cursor::new(b"0123456789".to_vec()));
    println!("| seek {} {}", sk.seek(SeekFrom::End(-3)).unwrap(), sk.stream_position().unwrap());
    sk.rewind().unwrap();
    let mut br: Box<Cursor<Vec<u8>>> = Box::new(Cursor::new(b"l1\nl2\n".to_vec()));
    let mut line = String::new();
    br.read_line(&mut line).unwrap();
    println!("| line {:?} buf {}", line, br.fill_buf().unwrap().len());
    br.consume(1);
}

#[test]
fn a05_os_string_comparisons() {
    use std::ffi::{OsStr, OsString};
    let owned = OsString::from("posix");
    let borrowed = OsStr::new("posix");
    println!("| {} {} {} {}", owned == borrowed, owned == *borrowed, borrowed == owned, *borrowed == owned);
    println!("| {}", owned == OsStr::new("right"));
}

#[cfg(target_arch = "aarch64")]
#[test]
fn a06_neon_pair_load() {
    use std::arch::aarch64::*;
    let bytes: [u8; 32] = core::array::from_fn(|i| i as u8);
    unsafe {
        let pair = vld1q_u8_x2(bytes.as_ptr());
        let lo: [u8; 16] = std::mem::transmute(pair.0);
        let hi: [u8; 16] = std::mem::transmute(pair.1);
        println!("| {} {} {} {}", lo[0], lo[15], hi[0], hi[15]);
    }
}
